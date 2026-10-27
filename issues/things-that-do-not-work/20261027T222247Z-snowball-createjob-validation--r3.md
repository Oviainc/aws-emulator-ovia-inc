Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# A transfer order whose S3Resources entry names no bucket is accepted: names_s3_bucket only checks the list is non-empty

_Filed: 2026-10-27T22:22:47Z_
_Operation: CreateJob | Class: validation_

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

For an IMPORT/EXPORT order CreateJob requires that the request name the buckets it transfers, and it answers InvalidInputCombinationException when it does not. That check is `crate::state::names_s3_bucket`, which asks only whether the `S3Resources` LIST is non-empty — it never looks at the `BucketArn` inside an entry. So an order carrying `Resources: {"S3Resources": [{}]}` (an entry that carries no `BucketArn`, which the model permits because `S3Resource` has no required member) passes the check, is stored, and is reported by DescribeJob as a valid `New` transfer order that names no bucket at all.

Three places in this same crate say that is exactly what must not happen:

1. The helper's own doc sentence claims it answers "whether `resources` names at least one Amazon S3 bucket for a transfer to move", and enumerates the three shapes that "all name no bucket" — but an entry with no BucketArn names no bucket either, and the code answers true for it.
2. The sibling half of the very same `if` block treats an EMPTY `RoleARN` as not supplied (`is_none_or(str::is_empty)`), so within one check the role is read by its value while the bucket is read by the presence of a wrapper object.
3. `state::named_resources::named_values` states the crate's rule for these members explicitly — a `S3Resource$BucketArn` left out or sent empty "names nothing" and "is not looked up" — so the resolution step silently skips such an entry too. The result is an order that neither names a bucket nor has any bucket resolved, answered 200.

The effect is the refusal the handler intends ("with no bucket there is nothing to transfer") is bypassed by adding one empty object to the list: `"S3Resources": []` is refused, `"S3Resources": [{}]` is accepted.

## Evidence

The helper, services/service-com-amazonaws-snowball-awsiesnowballjobmanagementservice/src/state.rs:213-224 — the doc states the rule, the body only measures the list:

    /// Whether `resources` names at least one Amazon S3 bucket for a transfer to move.
    ///
    /// An absent `Resources`, one carrying no `S3Resources` list and one carrying an empty
    /// list all name no bucket, so all three read the same way here.
    pub fn names_s3_bucket(resources: Option<&JobResource>) -> bool {
        resources.is_some_and(|resources| {
            resources
                .s3_resources
                .as_deref()
                .is_some_and(|buckets| !buckets.is_empty())
        })
    }

The caller, src/ops/create_job.rs:638-663 — the stated rule, and the two halves read differently:

    // An order that moves data between Amazon S3 and the device is the transfer of
    // named buckets under a named role (`crate::state::S3_TRANSFER_JOB_TYPES`), so it
    // cannot be placed without either: with no bucket there is nothing to transfer,
    ...
        let missing_transfer_member = if !crate::state::names_s3_bucket(input.resources.as_ref())
        {
            Some("the Amazon S3 buckets it transfers, in Resources")
        } else if input.role_arn.as_deref().is_none_or(str::is_empty) {
            Some("the RoleARN the transfer is performed under")

The crate's own rule for the same member, src/state/named_resources.rs:223-229 and 269-271:

    /// A member supplied EMPTY names nothing — `RoleARN`, `KmsKeyARN`, `S3Resource$BucketArn`,
    /// `LambdaResource$LambdaArn` and the two topic members are each modelled `length 0..`, so
    /// the empty string is a value they admit, and this service reads a member left out and a
    /// member sent empty the same way — so it is not looked up.
    ...
            let Some(value) = value.filter(|value| !value.is_empty()) else {
                continue;
            };

The model allows the entry: `smithy_query(query="shape", shape="S3Resource")` lists `BucketArn`, `KeyRange` and `TargetOnDeviceServices` with NO required member, so `{"S3Resources": [{}]}` is a well-formed request body.

Test coverage stops one row short — tests/all/job_order_validation.rs:335-346 pins only the shapes the helper does catch:

            ("no Resources member", None),
            ("Resources naming no bucket list", Some(json!({}))),
            (
                "an empty bucket list",
                Some(json!({ "S3Resources": [] })),
            ),

no row supplies an S3Resources entry that carries no BucketArn.

## Steps to Reproduce

1. POST / with X-Amz-Target: AWSIESnowballJobManagementService.CreateAddress and a deliverable address; keep the minted AddressId.
2. POST / ...CreateJob with {"JobType":"IMPORT","SnowballType":"EDGE_S","ShippingOption":"SECOND_DAY","AddressId":"<id>","RoleARN":"arn:aws:iam::123456789012:role/transfer","Resources":{"S3Resources":[{}]}} — observe HTTP 200 with a minted JobId.
3. POST / ...CreateJob with the same body but "Resources":{"S3Resources":[]} — observe the 400 InvalidInputCombinationException ("a IMPORT job must provide the Amazon S3 buckets it transfers, in Resources"), the refusal step 2 should also have produced.
4. POST / ...DescribeJob with the JobId from step 2 — observe JobMetadata.Resources.S3Resources[0] stored with no BucketArn: a New transfer order that names no bucket.
5. Read src/state.rs:217-224 and src/state/named_resources.rs:269-271 — the presence check counts the entry, the resolution step skips it, so nothing in the order ever names a bucket.
