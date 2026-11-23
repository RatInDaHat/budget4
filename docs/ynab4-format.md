# YNAB 4 budget format and sync protocol

What Budget4 (crates/budget4-core) relies on, worked out from how YNAB 4 (version 4.3.857) reads
and writes its files, and checked against a real budget. "Verified" means a test in `crates/budget4-core/tests` checks it against real files.

## Package layout

```
<Name>~<8 hex>.ynab4/
  Budget.ymeta                  {"formatVersion":"2","relativeDataFolderName":"dataN-XXXXXXXX","TED":…}
  Backup_<yyyy-mm-ddTHH-MM-SS>_<shortId>_<deviceGUID>.y4backup   zip: "<knowledge>.ynab4" (+ budgetSettings.ybsettings)
  <dataFolder>/
    devices/<letter>.ydevice    one per device (tab-indented JSON)
    <deviceGUID>/
      Budget.yfull              that device's full snapshot (tab-indented JSON)
      <startVV>_<letter>-<n>.ydiff   that device's change files (2-space JSON)
```

`TED` is YNAB's trial-expiry date; Budget4 never rewrites an existing `Budget.ymeta`.

## Records

Every record has `entityId`, `entityType`, `entityVersion` (`"<letter>-<counter>"`), `isTombstone`
(soft delete), `madeWithKnowledge`, `isResolvedConflict`. Field lists per type: `crates/budget4-core/src/schema.rs`.

* **Budget.yfull** omits these fields when `false`/`null`: isTombstone, FID, FITID, YNABID, source,
  checkNumber, cleared, dateEnteredFromSchedule, targetAccountId, transferTransactionId, importedPayee,
  flag, payeeId, memo, subTransactions, matchedTransactions, parentTransactionIdIfMatched, bankAccountId,
  note, priorBalance, madeWithKnowledge, isResolvedConflict. *Verified: every Budget.yfull in the test
  budget (2013–2026) re-serializes byte for byte.*
* **.ydiff items** carry every field, nulls included (a missing key means "unchanged" to the reader).
* **Aggregate children** live nested in the snapshot but travel as separate diff items, with the parent's
  array left out: `category` (masterCategoryId), `monthlyCategoryBudget` (parentMonthlyBudgetId),
  `payeeStringCondition` and `location` (parentPayeeId).
* **Inline children** (`subTransaction`, `matchedTransaction`) aren't publishable alone: changing one
  re-versions its parent transaction, which carries them inline.

IDs: transfer counterpart `<id>_T_<n>`; transfer payee `Payee/Transfer:<accountId>`; month `MB/YYYY-MM`;
category budget `MCB/YYYY-MM/<categoryId>`; scheduled occurrence `<scheduleId>_<yyyy-mm-dd>_<n>`; built-ins
`MasterCategory/__Hidden__|__Income__|__Internal__`, `Category/__ImmediateIncome__|__DeferredIncome__|__Split__`; Pre-YNAB debt: master
`MasterCategory/__PreYNABDebtMaster__` ("Pre-YNAB Debt") with one subcategory per credit card,
`Category/PreYNABDebt/<accountId>`.
Hidden categories move under `__Hidden__` renamed `"<Master> ` <Sub> ` <originalMasterId>"`.

## Versioning

Each device owns a letter. A local change increments the device's counter in the current knowledge
(version vector) and stamps the entity with it (and its inline parent, recursively), setting
`isResolvedConflict` to false.

## Publishing (VOToSyncDiffBridge.getSyncDiffToPublish)

Items: every publishable entity whose version is this device's and newer than the last publish.
`startVersion` = current knowledge with this device's element set to the last published counter;
`endVersion` = current knowledge. File name: `<startVersion>_<letter>-<end counter>.ydiff`.
Publishing happens before every full save. YNAB polls every 5 s and writes the full file at most every
10 minutes. After a full save the device file gets `knowledge`, `knowledgeInFullBudgetFile`,
`hasFullKnowledge: true`, `lastDataVersionFullyKnown: "4.2"`.

## Opening and importing

1. Register: find our `.ydevice` by GUID, else take the first free letter A–Z (letters are reused; the
   counter carries on from the knowledge vector) and create `devices/<letter>.ydevice` + `<GUID>/`.
2. Load the `Budget.yfull` of the device whose `knowledgeInFullBudgetFile` sorts highest.
3. Import diffs from all registered device folders, repeatedly: skip if knowledge ⊇ end; import if
   knowledge ⊇ start (then knowledge ∪= end); otherwise retry later ("broken chain" if never).
4. Merge each item (compareEntities): if the item's knowledge (its `madeWithKnowledge` or the diff's
   `endVersion`) covers our version, take it; if our knowledge covers its version, keep ours; else conflict:
   both already resolved → lower device letter wins silently; tombstone vs not → tombstone wins;
   otherwise lower device letter wins. Non-silent resolutions are re-stamped with `isResolvedConflict: true`.

*Verified: for 5 pairs of historical snapshots (≈1,300 diffs, including 71 genuine concurrent conflicts),
older snapshot + diffs reproduces the newer snapshot entity for entity.*

## Budget math (data.MonthlyBudget …)

Per category and month: `spent = −Σ amounts` of on-budget transactions in the category (split lines
count individually); `priorBalance = prev.balance` if it is ≥ 0 or the previous month's handling is
`Confined`, else 0; `balance = priorBalance + budgeted − spent`. Overspending handling is inherited from
the previous month when not set (default `AffectsBuffer`). Per month: `totalCarriedOverspending` = Σ negative
balances with `AffectsBuffer`; `startingBuffer = prev.available + prev.totalCarriedOverspending +
deferredIncomeFromPreviousMonth`; `available = startingBuffer + immediateIncome − totalBudgeted`. Every add
rounds to the currency's decimals. Cleared status never affects the budget.

*Verified: matches a YNAB 4 Budget.csv export for every category and month before the export date
(≈11,800 rows).*

Account balances: cleared includes reconciled; working = cleared + uncleared.

## Importing (ImporterOFX, ImportUtilities, TransactionList.match)

From YNAB 4.3.857's bytecode; the import-id format is checked against a real budget.

**Reading OFX/QFX/QBO.** Every line is split before each `<`, pieces are trimmed, text that doesn't
start with `<` is appended to the previous piece with a space, CDATA is unwrapped, and `&amp;` `&lt;`
`&gt;` and non-breaking spaces are decoded. Tags are then read in order (SGML and XML files alike):
`SIGNONMSGSRS/FID`, and per `STMTRS` / `CCSTMTRS` / `INVSTMTRS` the `BANKID`, `ACCTID`, `ACCTTYPE` and
each `STMTTRN`'s `DTPOSTED` (first 8 digits), `TRNAMT`, `FITID`, `NAME`/`PAYEE`, `MEMO`, `CHECKNUM`.
A `REFNUM` is the check number only on `TRNTYPE` CHECK. The mapping key ("fid") is SIGNON's FID when
there is one, else the statement's BANKID. Statements with no transactions are dropped.

**Import ids.** `YNAB:<amount>:<date>:<n>` with the amount formatted like `-1,234.56` and `n` counting
from 1 per amount+date within the file; the transaction's entityId is `Transaction/<that>` (plus
`_0`, `_1`… if taken). *Verified: Budget4 reproduces the id of 98% of the never-matched imported
transactions in the test budget (the rest were edited after import); every entityId follows its id.*

**Importing into an account** (`importAccount`), per file transaction:
1. Skip it if a live transaction in the account has the same YNABID, or the same FITID with the same
   amount within 4 days (that one is given the new YNABID).
2. Leave it out if it's dated before the account's start (its starting balance) unless asked to
   include those; then the starting balance moves earlier and changes by minus their total.
3. Payee: the file's NAME (or MEMO when "swap payee and memo" is on, or when the file has memos but
   no payees). An enabled payee with that exact name (any case) wins, else the first enabled payee by
   name whose rename rules (`Is` / `Contains` / `StartsWith` / `EndsWith`, any case) fit, else a new
   payee. An existing payee's auto-fill category is used unless it's the split category. The text
   is kept as `importedPayee`.
4. Setting `source: "Imported"` also sets `cleared: "Cleared"`; `accepted` is false.
5. Matching (`findMatchingTransaction(t, false, true, true, true, true, batch)`): candidates are live
   transactions in the account with the same amount, dated within 10 days (60 when both check numbers
   are numeric and equal), without a YNABID or FITID, not matched already and not imported in this
   batch. One candidate: it. Several: those with the same check number, else the closest dates, then
   the smallest Damerau-Levenshtein distance between payee names (last one on ties).
6. A match (`TransactionList.match`) creates one transaction under the bank's id with the entered
   transaction's date, payee (and transfer), category, memo, flag and splits, the bank's
   FITID/YNABID/source/importedPayee, the larger cleared status, the entered check number if any, and
   `accepted: false`. Both originals are kept in `matchedTransactions` as `matchedTransaction` records
   with ids `MT/<merged id>/MatchedTransaction/<original id>` (bank first) and
   `parentTransactionIdIfMatched`. The entered original is tombstoned.
7. Approving (`Transaction.set accepted`) empties `matchedTransactions` (the array stays, `[]`) and
   approves the other side of a transfer. Unmatching restores the entered transaction and turns the
   merged one back into the bank's.

**Account mappings** (`AccountMappingManager`): `fid`, `shortenedAccountId` (last 4 characters of
ACCTID), `salt` (32 random bytes, base64) and `hash` = base64(sha256(UTF-8 of ACCTID + salt)), so the
account number itself is never stored. Lookup recomputes the hash with each mapping's salt and
compares fids; Budget4 compares fids with whitespace trimmed (some saved by YNAB 4 end in a space).
Mappings hold `targetYNABAccountId`, `shouldFlipPayeesMemos`, `shouldImportMemos` and `skipImport`.

**CSV.** YNAB 4 only read its own layout (Date, Payee, Category, Memo, Outflow, Inflow or Amount).
Budget4 maps any bank's columns instead (guessed from the header, remembered per account in
`~/.budget4/config.json`) and gives the rows the same import ids, so re-importing a CSV skips what's
already there.

