// SPDX-License-Identifier: CC0-1.0

//! What AUTHORIZES one request this service is serving: the bucket's own policy, and the
//! account the request says owns the bucket.
//!
//! A bucket policy is a RESOURCE-BASED policy, so it is in force on every request against
//! the bucket's resources — a read, a write, a copy's read of its source — and not only on
//! the public ones. "An explicit deny in any policy overrides any allow" (IAM's own
//! evaluation order), so a statement about the request whose `Effect` is `Deny` refuses the
//! call before it does anything, whatever an identity-based policy would have granted.
//!
//! The decision itself is the tree's ONE evaluation of IAM policy documents
//! (`service_common::iam_authorization`), which every other emulated service that enforces a
//! resource policy answers with. What this module adds is the half only S3 knows:
//!
//! * the CONTEXT the request carries — the condition keys a statement may be written over,
//!   taken from the request's own members ([`ObjectWriteContext`]) — so a clause over one of
//!   them is DECIDED rather than unknown, which is what makes a policy that conditions a
//!   write on an encryption header enforceable at all;
//! * the KEYS this service declares it models ([`PUBLISHED_CONDITION_KEYS`]): absence of one
//!   of these from the context means "the request carries no value", which IAM reads under a
//!   negated operator, an `IfExists` spelling and a `Null` clause, while absence of any other
//!   key leaves the statement undecided;
//! * the SENTENCE a refusal is worded in, which for an explicit deny is the one IAM
//!   publishes for that decision ([`crate::state::explicit_resource_policy_denial`]).
//!
//! Only a deny this evaluation could DECIDE refuses. A clause over a key this service does
//! not answer leaves its statement undecided, and reading an undecided `Deny` as a refusal
//! would refuse the bucket owner's own calls on every bucket whose policy conditions anything
//! this emulator does not model — so such a deny stands aside. A bucket carrying no policy
//! denies nothing.

use ::service_common::iam_authorization::{
    account_principals, resource_outcome_prepared, Decision, PreparedDocument, Request,
};

use crate::state::{
    self, caller_principal_arn, explicit_resource_policy_denial, Bucket, Object,
    ACCESS_DENIED_CODE, ACCESS_DENIED_MESSAGE, ACCESS_DENIED_STATUS, GET_OBJECT_ACTION,
    LIST_BUCKET_ACTION,
};

/// The condition key saying whether the request arrived over a secure transport: "whether
/// the request was sent using SSL" (AWS General Reference, *Global condition context keys*).
///
/// This service ANSWERS it rather than leaving it open, because it knows: the emulator serves
/// every endpoint over TLS, so every request it judges arrived that way.
const SECURE_TRANSPORT_KEY: &str = "aws:SecureTransport";

/// The value that key carries, as IAM's boolean literals spell it.
const SECURE_TRANSPORT_VALUE: &str = "true";

/// The condition key an object write's `x-amz-acl` header answers (Amazon S3 User Guide,
/// *Actions, resources, and condition keys for Amazon S3*: `s3:x-amz-acl`).
const ACL_KEY: &str = "s3:x-amz-acl";

/// The condition key an object write's `x-amz-storage-class` header answers
/// (`s3:x-amz-storage-class`, same published list).
const STORAGE_CLASS_KEY: &str = "s3:x-amz-storage-class";

/// The condition key an object write's `x-amz-server-side-encryption` header answers
/// (`s3:x-amz-server-side-encryption`, same published list) — the key a policy that makes
/// encryption mandatory is written over.
const SERVER_SIDE_ENCRYPTION_KEY: &str = "s3:x-amz-server-side-encryption";

/// The condition key an object write's `x-amz-server-side-encryption-aws-kms-key-id` header
/// answers (`s3:x-amz-server-side-encryption-aws-kms-key-id`, same published list).
const KMS_KEY_ID_KEY: &str = "s3:x-amz-server-side-encryption-aws-kms-key-id";

/// The condition key a copy's `x-amz-copy-source` header answers (`s3:x-amz-copy-source`,
/// same published list).
const COPY_SOURCE_KEY: &str = "s3:x-amz-copy-source";

/// The condition key a copy's `x-amz-metadata-directive` header answers
/// (`s3:x-amz-metadata-directive`, same published list).
const METADATA_DIRECTIVE_KEY: &str = "s3:x-amz-metadata-directive";

/// Every condition key a request judged here carries a value for when its member is present,
/// and so DECLARES this service models — the keys whose absence from the context is decided
/// ("the request carries none") rather than unknown. Every other key keeps the undecided
/// reading, which refuses nothing.
pub const PUBLISHED_CONDITION_KEYS: &[&str] = &[
    SECURE_TRANSPORT_KEY,
    ACL_KEY,
    STORAGE_CLASS_KEY,
    SERVER_SIDE_ENCRYPTION_KEY,
    KMS_KEY_ID_KEY,
    COPY_SOURCE_KEY,
    METADATA_DIRECTIVE_KEY,
];

/// The members of an object write a bucket policy may be written over, as the request carried
/// them.
///
/// One value per published S3 request-header condition key, `None` for a header the request
/// does not carry — which is itself part of the decision: a policy denying a write "unless"
/// it names an encryption algorithm is a clause that HOLDS exactly when the header is absent.
#[derive(Debug, Default, Clone, Copy)]
pub struct ObjectWriteContext<'a> {
    /// `x-amz-acl` — the canned ACL the write names.
    pub acl: Option<&'a str>,
    /// `x-amz-storage-class` — the class the write asks the object to be stored in.
    pub storage_class: Option<&'a str>,
    /// `x-amz-server-side-encryption` — the algorithm the write asks for.
    pub server_side_encryption: Option<&'a str>,
    /// `x-amz-server-side-encryption-aws-kms-key-id` — the KMS key the write names.
    pub kms_key_id: Option<&'a str>,
    /// `x-amz-copy-source` — the object a copy reads, for the write half of a copy.
    pub copy_source: Option<&'a str>,
    /// `x-amz-metadata-directive` — whether a copy keeps or replaces the source's metadata.
    pub metadata_directive: Option<&'a str>,
}

impl ObjectWriteContext<'_> {
    /// The request's condition context: the secure transport every endpoint of this emulator
    /// serves, plus one pair per member the request carried.
    pub fn conditions(&self) -> Vec<(String, String)> {
        let mut context = vec![(
            SECURE_TRANSPORT_KEY.to_string(),
            SECURE_TRANSPORT_VALUE.to_string(),
        )];
        for (key, value) in [
            (ACL_KEY, self.acl),
            (STORAGE_CLASS_KEY, self.storage_class),
            (SERVER_SIDE_ENCRYPTION_KEY, self.server_side_encryption),
            (KMS_KEY_ID_KEY, self.kms_key_id),
            (COPY_SOURCE_KEY, self.copy_source),
            (METADATA_DIRECTIVE_KEY, self.metadata_directive),
        ] {
            if let Some(value) = value {
                context.push((key.to_string(), value.to_string()));
            }
        }
        context
    }
}

/// The condition context a request that carries none of the write members answers with: the
/// secure transport alone, which is what a read's own clauses are judged against.
pub fn read_conditions() -> Vec<(String, String)> {
    ObjectWriteContext::default().conditions()
}

/// Why the bucket's `policy` refuses the calling principal `action` on `resource` — the
/// sentence IAM publishes for an explicit deny in a resource-based policy — or `None` when it
/// leaves the call alone.
///
/// The principal is the one a credentialed request here speaks for
/// ([`caller_principal_arn`]) together with the spellings that name its whole account, since
/// a statement denying the account refuses each of its principals. Only a DENY is reported:
/// this emulator serves one account and the caller is that account, so a document that says
/// nothing about the request leaves the bucket's own owner able to act on it.
pub fn policy_denial(
    policy: Option<&str>,
    action: &str,
    resource: &str,
    conditions: &[(String, String)],
) -> Option<String> {
    let policy = policy?;
    let caller = caller_principal_arn();
    let mut principals = vec![caller.clone()];
    principals.extend(account_principals(&caller));
    let outcome = resource_outcome_prepared(
        &PreparedDocument::read(policy),
        &Request::new(action, resource, &principals)
            .with_context(conditions)
            .with_published_keys(PUBLISHED_CONDITION_KEYS),
    );
    // A key the decision needed and this service does not model leaves the statement
    // undecided, which refuses nothing; a key it DOES model is decided by its absence, so a
    // report of one says only that the request carried no value for it.
    let undecided = outcome
        .required_context_keys
        .iter()
        .any(|key| !models(key));
    (outcome.decision == Decision::Denied && !undecided)
        .then(|| explicit_resource_policy_denial(action, resource))
}

/// Whether this service ANSWERS the condition key `key`, compared the way a condition key's
/// name is compared everywhere ([`PUBLISHED_CONDITION_KEYS`]).
fn models(key: &str) -> bool {
    PUBLISHED_CONDITION_KEYS
        .iter()
        .any(|known| known.eq_ignore_ascii_case(key))
}

/// Why S3 refuses a read of `key` — the bytes or the metadata of the version `addressed`
/// names, `None` for an address the bucket holds no object at — to the CREDENTIALED caller,
/// or `None` when the bucket's policy leaves the read to it.
///
/// WHICH refusal it is depends on what the address resolved to, and both are worded as the
/// explicit deny they are, about the action the policy denied:
///
/// * a version the read DID resolve, whose read the policy denies, is refused the read itself
///   — [`GET_OBJECT_ACTION`] on that object;
/// * an address holding NOTHING is answered by what the caller may learn about the bucket's
///   contents at all: "if the object that you request doesn't exist, the error that Amazon S3
///   returns depends on whether you also have the `s3:ListBucket` permission. If you have the
///   `s3:ListBucket` permission on the bucket, Amazon S3 returns an HTTP status code 404 Not
///   Found error. If you don't have the `s3:ListBucket` permission, Amazon S3 returns an HTTP
///   status code 403 Access Denied error" (`GetObject`'s own documentation) — so a policy
///   denying [`LIST_BUCKET_ACTION`] refuses the read instead of reporting the key absent, and
///   one that does not leaves the 404 to be answered.
///
/// Asked BEFORE anything about the key is reported, which is what keeps a refusal from saying
/// whether the object is there.
pub fn caller_object_read_refusal(
    bucket: &Bucket,
    name: &str,
    key: &str,
    addressed: Option<&Object>,
) -> Option<String> {
    let conditions = read_conditions();
    if addressed.is_some() {
        return policy_denial(
            bucket.policy.as_deref(),
            GET_OBJECT_ACTION,
            &state::object_arn(name, key),
            &conditions,
        );
    }
    policy_denial(
        bucket.policy.as_deref(),
        LIST_BUCKET_ACTION,
        &bucket.arn(name),
        &conditions,
    )
}

/// Why a request naming `expected` as the account that owns the bucket it addresses is
/// refused, or `None` for one that names the owner or names no account at all.
///
/// "The account ID of the expected bucket owner. If the account ID that you provide does not
/// match the actual owner of the bucket, the request fails with the HTTP status code 403
/// Forbidden (access denied)" — the `ExpectedBucketOwner` member every bucket and object
/// operation of this model binds to `x-amz-expected-bucket-owner`. This emulator serves ONE
/// account, so that account owns every bucket it holds and any other value names an owner no
/// bucket here has.
///
/// One place, so the confused-deputy guard is the same answer on every operation that carries
/// the header, and judged BEFORE the store is reached: a mismatched guard says nothing about
/// what the bucket holds.
pub fn expected_bucket_owner_refusal<E>(
    expected: Option<&str>,
) -> Option<crate::scaffolded::Scaffolded<E>> {
    expected
        .filter(|account| *account != ::engine::emulator_account_id())
        .map(|_| {
            crate::scaffolded::Scaffolded::unmodeled(
                ACCESS_DENIED_CODE,
                ACCESS_DENIED_STATUS,
                ACCESS_DENIED_MESSAGE,
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A document denying `s3:PutObject` to everyone unless the write names the encryption
    /// algorithm `required`, which is how a bucket makes encryption mandatory.
    fn deny_unencrypted(required: &str) -> String {
        format!(
            "{{\"Version\":\"2012-10-17\",\"Statement\":[{{\"Effect\":\"Deny\",\
             \"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
             \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
             {{\"StringNotEquals\":{{\"s3:x-amz-server-side-encryption\":\"{required}\"}}}}}}]}}"
        )
    }

    /// The clause a policy that requires the header's PRESENCE is written as instead.
    const DENY_WITHOUT_THE_HEADER: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
         [{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
         {\"Null\":{\"s3:x-amz-server-side-encryption\":\"true\"}}}]}";

    /// A condition over a key this service does not answer leaves the deny undecided.
    const DENY_ON_AN_UNMODELLED_KEY: &str = "{\"Version\":\"2012-10-17\",\"Statement\":\
         [{\"Effect\":\"Deny\",\"Principal\":\"*\",\"Action\":\"s3:PutObject\",\
         \"Resource\":\"arn:aws:s3:::subject/*\",\"Condition\":\
         {\"StringEquals\":{\"s3:ExistingObjectTag/stage\":\"production\"}}}]}";

    /// The write's own members decide a clause written over them: the rows differ along which
    /// algorithm the write names (none, the required one, another one) and which form the
    /// clause takes (a negated comparison, an existence test, a key nothing here answers).
    #[test]
    fn a_clause_over_a_request_header_is_decided_by_that_header() {
        let rows: [(&str, String, Option<&str>, bool); 7] = [
            (
                "no encryption header against a policy requiring aws:kms",
                deny_unencrypted("aws:kms"),
                None,
                true,
            ),
            (
                "the required algorithm",
                deny_unencrypted("aws:kms"),
                Some("aws:kms"),
                false,
            ),
            (
                "another algorithm than the required one",
                deny_unencrypted("aws:kms"),
                Some("AES256"),
                true,
            ),
            (
                "no encryption header against a policy requiring AES256",
                deny_unencrypted("AES256"),
                None,
                true,
            ),
            (
                "the required AES256",
                deny_unencrypted("AES256"),
                Some("AES256"),
                false,
            ),
            (
                "no encryption header against a policy requiring its presence",
                DENY_WITHOUT_THE_HEADER.to_string(),
                None,
                true,
            ),
            (
                "a deny over a condition key this service does not answer",
                DENY_ON_AN_UNMODELLED_KEY.to_string(),
                None,
                false,
            ),
        ];
        for (case, policy, algorithm, refused) in rows {
            let context = ObjectWriteContext {
                server_side_encryption: algorithm,
                ..ObjectWriteContext::default()
            };
            let denial = policy_denial(
                Some(&policy),
                "s3:PutObject",
                "arn:aws:s3:::subject/reports/held.csv",
                &context.conditions(),
            );
            assert_eq!(
                denial.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {denial:?}"
            );
        }
    }

    /// The guard compares the account the request names against the one account this emulator
    /// serves: the rows differ along whose account it names.
    #[test]
    fn the_expected_bucket_owner_guard_admits_only_the_owner() {
        let owner = ::engine::emulator_account_id().to_string();
        let rows: [(&str, Option<&str>, bool); 4] = [
            ("no account named at all", None, false),
            ("the account this emulator serves", Some(&owner), false),
            ("another account of the same shape", Some("123456789012"), true),
            ("a value that is no account id", Some("not-an-account"), true),
        ];
        for (case, expected, refused) in rows {
            let refusal = expected_bucket_owner_refusal::<()>(expected);
            assert_eq!(
                refusal.is_some(),
                refused,
                "case {case:?}: expected refused={refused}, got {:?}",
                refusal.is_some()
            );
        }
    }
}
