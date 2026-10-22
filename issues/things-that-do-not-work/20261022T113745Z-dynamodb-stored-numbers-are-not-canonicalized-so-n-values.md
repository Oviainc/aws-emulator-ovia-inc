Issue-Source: trace-sweep-divergence
Issue-Trace: dynamodb-number-canonical-stored-form
Issue-Step: task#10
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: dynamodb-number-canonical-stored-form ddbdepth-alltypes-roundtrip
Issue-Shape-Id: com.amazonaws.dynamodb#DynamoDB_20120810
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v0.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v1.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v10.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v11.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v12.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v13.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v14.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v15.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v16.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v2.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v3.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v4.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v5.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v6.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v7.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v8.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.v9.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.a_l.L[*].M.n.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.sk.N
Issue-Key: dynamodb|GetItem|2xx->2xx|$.Item.ns.NS[*]
Issue-Commit: 38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5

# Stored numbers are not canonicalized, so N values round-trip in their submitted form

> _Note: this issue was identified against the code base at commit `38aaeb6c250810a0cf0ab5b6ac35c7222e2099c5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutItem

## Description
DynamoDB stores an N attribute in a canonical decimal form; the emulator stores the literal string the client sent. This bundle has 4 causes; all 4 must be closed.

Cause 1 of 4: top-level N attributes are returned in their submitted lexical form.
AWS: Smithy: AttributeValue member N is a String ('Numbers are sent across the network to DynamoDB as strings, to maximize compatibility across languages'); the recording shows every accepted literal normalized on read - '+7' -> '7', '+1.5' -> '1.5', '+0'/'-0'/'+0.0' -> '0', '+1e2'/'1e+2' -> '100', '1.5E+3' -> '1500', '+.5'/'.5' -> '0.5', '5.' -> '5', '1.e4' -> '10000', '00042' -> '42', '+1e-2' -> '0.01', '1.23E10' -> '12300000000', '0100.50' -> '100.5', '-0.500' -> '-0.5'.
Emulator: GetItem echoes the submitted text verbatim for all 17 attributes.
Why: a client comparing a stored N to a locally formatted number, or using it as a map key/ETag, mismatches; two writes of the same value compare unequal; signs like '-0' leak where AWS has '0'.
Fix: the number normalization in the attribute-value parser/state writer shared by the item write path (canonicalize on write, not on read).
Traces: dynamodb-number-canonical-stored-form#task#10.

Cause 2 of 4: numbers nested inside L/M are not canonicalized. AWS: as Cause 1, applied recursively to AttributeValue L and M members (recording: '1e2' stored as '100' inside a list of maps). Emulator: '1e2' is returned unchanged. Why: the same mismatch, harder to spot because it is nested. Fix: the normalization must recurse through L/M in the attribute-value parser. Traces: ddbdepth-alltypes-roundtrip#task#4.

Cause 3 of 4: a numeric SORT KEY is not canonicalized. AWS: as Cause 1 for key attributes - a key submitted as '+5' reads back as '5'. Emulator: '+5'. Why: key identity is at stake - '+5' and '5' must address the same item, so a client that reads one form and writes the other creates or misses items. Fix: key normalization on every path (Get/Put/Update/Delete/Query key handling) before the item is addressed. Traces: dynamodb-number-canonical-stored-form#task#12.

Cause 4 of 4: number set (NS) members are not canonicalized (and therefore not deduplicated against their canonical twins). AWS: as Cause 1 per member; the recording shows ['+1','02','3.0','1e1'] stored as ['1','2','3','10']. Emulator: the submitted members are returned verbatim. Why: a set can hold two spellings of the same number, which AWS treats as a duplicate-member error, so set semantics diverge. Fix: canonicalize each NS member in the attribute-value parser before set-membership checks. Traces: dynamodb-number-canonical-stored-form#task#14.

## Evidence
Representative: fixture dynamodb-number-canonical-stored-form step task#10, GetItem, expected 200 / actual 200, 17 diffs including:
  $.Item.v0.N expected '7' got '+7'; v2/v3/v4 expected '0' got '+0'/'-0'/'+0.0'; v5/v6 expected '100' got '+1e2'/'1e+2'; v7 expected '1500' got '1.5E+3'; v8/v9 expected '0.5' got '+.5'/'.5'; v10 expected '5' got '5.'; v11 expected '10000' got '1.e4'; v12 expected '42' got '00042'; v13 expected '0.01' got '+1e-2'; v14 expected '12300000000' got '1.23E10'; v15 expected '100.5' got '0100.50'; v16 expected '-0.5' got '-0.500'.
Other causes: ddbdepth-alltypes-roundtrip#task#4 - $.Item.a_l.L[0].M.n.N expected '100' got '1e2'; dynamodb-number-canonical-stored-form#task#12 - $.Item.sk.N expected '5' got '+5'; #task#14 - $.Item.ns.NS members expected ['1','2','3','10'] got ['+1','02','3.0','1e1'].

## Steps to Reproduce
1. Replay the wire corpus.
2. Fixture dynamodb-number-canonical-stored-form, step task#10: PutItem with N attributes written as '+7', '1e2', '.5', '00042', '-0.500', then GetItem and compare the returned strings.
3. Same fixture, step task#12 (numeric sort key '+5') and step task#14 (NS members '+1','02','3.0','1e1').
4. Fixture ddbdepth-alltypes-roundtrip, step task#4: an N nested inside L/M.
