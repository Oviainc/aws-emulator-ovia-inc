Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 7bcaea858dd8788316443d36e8dbe675c4b82465

# encryptionSpecification accepts a kmsKeyIdentifier together with AWS_OWNED_KMS_KEY, although the crate refuses every mirror case of "this member belongs to the other mode"

_Filed: 2026-10-29T13:15:14Z_
_Operation: UpdateTable | Class: validation_

> _Note: this issue was identified against the code base at commit `7bcaea858dd8788316443d36e8dbe675c4b82465`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate has four resolvers for "a member that only belongs to one value of a sibling discriminator": `resolve_capacity` (capacity units belong to `PROVISIONED`), `resolve_auto_scaling` (auto scaling belongs to `PROVISIONED`), `resolve_replica_specifications` (a per-Region read-capacity override belongs to `PROVISIONED`) and `resolve_encryption` (`kmsKeyIdentifier` belongs to `CUSTOMER_MANAGED_KMS_KEY`). The first three refuse BOTH halves of the rule — the member missing where it is required AND the member supplied where it does not apply — and `resolve_capacity`'s own doc comment says so in as many words ("The model expresses neither half of that, so both are refused here"). `resolve_encryption` is the lone exception: it refuses only the missing half.

So `UpdateTable` with `encryptionSpecification = {"type": "AWS_OWNED_KMS_KEY", "kmsKeyIdentifier": "arn:aws:kms:..."}` is accepted with 200, the whole specification is stored verbatim on the table (`table.encryption_specification = Some(encryption)`), and `GetTable` then reports a table that is encrypted with the Amazon-owned key while naming a customer-managed KMS key — a combination this crate's own test says cannot exist, and one no accepted request can otherwise produce: the sibling row of that same test pins that moving to `AWS_OWNED_KMS_KEY` DROPS the stored key identifier because "the key identifier is input for the customer managed type only".

The gap is in the shared resolver `crate::state::resolve_encryption`, so the same request shape is accepted by `CreateTable` (`encryptionSpecification`) and by `RestoreTable` (`encryptionSpecificationOverride`) as well; a fix in that one resolver covers all three callers. It is a different gap from the open `CreateTable`/validation issue about a keyspace that is still `CREATING`/`UPDATING`.

Note this is NOT about the enum values (both are modelled and the boundary validator enforces the value set) and not about a cap — it is the member-combination rule the crate already enforces three times over.

## Evidence

The gap — `services/service-com-amazonaws-keyspaces-keyspacesservice/src/state.rs:2230-2247` refuses only the "requires" half and returns the caller's structure unchanged:
```
pub fn resolve_encryption(
    supplied: Option<EncryptionSpecification>,
) -> Result<EncryptionSpecification, String> {
    let Some(encryption) = supplied else {
        return Ok(EncryptionSpecification {
            r#type: DEFAULT_ENCRYPTION_TYPE.to_string(),
            kms_key_identifier: None,
        });
    };
    if encryption.r#type == CUSTOMER_MANAGED_ENCRYPTION_TYPE
        && encryption.kms_key_identifier.is_none()
    {
        return Err(format!(
            "encryptionSpecification with type '{CUSTOMER_MANAGED_ENCRYPTION_TYPE}' requires kmsKeyIdentifier"
        ));
    }
    Ok(encryption)
}
```

The mirror sibling in the SAME module that refuses both halves — `src/state.rs:2172-2179` (its own doc) and `:2206-2219` (the body):
```
/// The capacity units belong to `PROVISIONED` and to nothing else: that mode
/// "requires `readCapacityUnits` and `writeCapacityUnits` as input"
/// (`CapacitySpecification$throughputMode`), and no other mode takes them. The
/// model expresses neither half of that, so both are refused here, naming the
/// member the caller has to fix.
```
```
    } else {
        let supplied_units: Vec<&str> = units
            .iter()
            .filter(|(_, present)| *present)
            .map(|(name, _)| *name)
            .collect();
        if !supplied_units.is_empty() {
            return Err(format!(
                "capacitySpecification with throughputMode '{}' does not take {}; they are input for '{PROVISIONED_THROUGHPUT_MODE}' only",
                capacity.throughput_mode,
                supplied_units.join(" or ")
            ));
        }
    }
```
the same "does not apply" refusal twice more — `src/state.rs:2462-2468`:
```
    if capacity.throughput_mode != PROVISIONED_THROUGHPUT_MODE {
        return Err(format!(
            "autoScalingSpecification is the auto scaling setting of a table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; \
             capacitySpecification.throughputMode is '{}'",
            capacity.throughput_mode
        ));
    }
```
and `src/state.rs:2551-2562`:
```
        let overrides_read_capacity = specification.read_capacity_units.is_some()
            || specification.read_capacity_auto_scaling.is_some();
        let throughput_mode = capacity.map(|capacity| capacity.throughput_mode.as_str());
        if overrides_read_capacity && throughput_mode != Some(PROVISIONED_THROUGHPUT_MODE) {
            return Err(format!(
                "replicaSpecifications entry for Region '{}' overrides the read capacity of a \
                 table in capacity mode '{PROVISIONED_THROUGHPUT_MODE}'; ...
```

The crate stating the encryption rule in its own words, in the test that pins the opposite direction — `tests/all/update_table.rs:262-275`:
```
        Case {
            // CUSTOMER_MANAGED_KMS_KEY -> AWS_OWNED_KMS_KEY, the other direction: the
            // key identifier is input for the customer managed type only, so it goes.
            name: "encryption_to_aws_owned",
            create: || {
                json!({"encryptionSpecification": {
                    "type": "CUSTOMER_MANAGED_KMS_KEY",
                    "kmsKeyIdentifier": KMS_KEY_ARN,
                }})
            },
            update: || json!({"encryptionSpecification": {"type": "AWS_OWNED_KMS_KEY"}}),
            touched: &["encryptionSpecification"],
            expect: || vec![("/encryptionSpecification", json!({"type": "AWS_OWNED_KMS_KEY"}))],
```

The model sentence the rule comes from (`smithy_query(query="doc",shape="com.amazonaws.keyspaces#EncryptionSpecification",member="type")`): "`type:AWS_OWNED_KMS_KEY` - This key is owned by Amazon Keyspaces. ... `type:CUSTOMER_MANAGED_KMS_KEY` - This key is stored in your account and is created, owned, and managed by you. This option requires the `kms_key_identifier` of the KMS key in Amazon Resource Name (ARN) format as input."

Where the unchecked structure is stored and reported — `src/ops/update_table.rs:305-310` (resolve) and `:729-731` (store):
```
        let encryption_specification = match input.encryption_specification {
            Some(supplied) => {
                Some(crate::state::resolve_encryption(Some(supplied)).map_err(invalid)?)
            }
            None => None,
        };
```
```
        if let Some(encryption) = encryption_specification {
            table.encryption_specification = Some(encryption);
        }
```
and `src/ops/get_table.rs:295`:
```
            encryption_specification: table.encryption_specification.clone(),
```
The same resolver is called by `src/ops/create_table.rs:298-299` and `src/ops/restore_table.rs:267-272`, so all three request members accept the combination.

Test coverage: the two pinned encryption rows are `tests/all/update_table.rs:240-261` (AWS_OWNED -> CUSTOMER_MANAGED with the key) and `:262-275` (quoted above), plus the "requires" refusal at `tests/all/update_table.rs:680-684` and `tests/all/keyspace_table_lifecycle.rs:1088-1093`. None of them supplies `kmsKeyIdentifier` alongside `AWS_OWNED_KMS_KEY`, so nothing pins the accepted case and nothing constrains a fix.

## Steps to Reproduce

1. Start the Keyspaces emulator
2. CreateKeyspace {"keyspaceName":"app"} and wait out the transient CREATING span
3. CreateTable {"keyspaceName":"app","tableName":"t","schemaDefinition":{"allColumns":[{"name":"id","type":"text"}],"partitionKeys":[{"name":"id"}]}} and wait for ACTIVE — the table holds the modelled default {"type":"AWS_OWNED_KMS_KEY"}
4. UpdateTable {"keyspaceName":"app","tableName":"t","encryptionSpecification":{"type":"AWS_OWNED_KMS_KEY","kmsKeyIdentifier":"arn:aws:kms:us-west-2:000000000000:key/aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee"}} — actual: 200 with the table ARN; expected (by the rule resolve_capacity/resolve_auto_scaling/resolve_replica_specifications apply for their own mode-bound members, and the rule tests/all/update_table.rs:263-264 states for this one): 400 ValidationException saying kmsKeyIdentifier is input for CUSTOMER_MANAGED_KMS_KEY only
5. GetTable {"keyspaceName":"app","tableName":"t"} — actual: "encryptionSpecification":{"type":"AWS_OWNED_KMS_KEY","kmsKeyIdentifier":"arn:aws:kms:..."}, a combination no other accepted request can produce (the test row at tests/all/update_table.rs:262-275 pins that a move to AWS_OWNED_KMS_KEY drops the key identifier)
6. Compare with the mirror case, which IS refused: UpdateTable {"keyspaceName":"app","tableName":"t","capacitySpecification":{"throughputMode":"PAY_PER_REQUEST","readCapacityUnits":5,"writeCapacityUnits":5}} -> 400 "capacitySpecification with throughputMode 'PAY_PER_REQUEST' does not take readCapacityUnits or writeCapacityUnits; they are input for 'PROVISIONED' only"
7. Repeat step 4 as CreateTable {...,"encryptionSpecification":{"type":"AWS_OWNED_KMS_KEY","kmsKeyIdentifier":"arn:aws:kms:..."}} and as RestoreTable {...,"encryptionSpecificationOverride":{same}} — both accept it the same way, since all three call crate::state::resolve_encryption
