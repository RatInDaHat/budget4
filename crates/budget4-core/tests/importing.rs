//! Bank file import on throwaway budgets: OFX parsing, duplicate detection, matching to entered
//! transactions, rename rules, approve / unmatch, account mappings, and syncing the result.

mod common;

use budget4_core::create::create_budget;
use budget4_core::editor::{AccountInput, Editor, TransactionInput};
use budget4_core::importing::{csv_transactions, guess_csv_mapping, parse_ofx, read_csv, ImportOptions};
use budget4_core::json::{parse, ObjExt};
use budget4_core::package::{new_guid, read_json, BudgetPackage, OpenOptions};
use budget4_core::schema::blank_entity;
use budget4_core::store::Store;
use serde_json::Value;

const SGML: &str = "OFXHEADER:100\r\nDATA:OFXSGML\r\nVERSION:102\r\nENCODING:USASCII\r\nCHARSET:1252\r\n\r\n\
<OFX><SIGNONMSGSRSV1><SONRS><STATUS><CODE>0<SEVERITY>INFO</STATUS><DTSERVER>20261106\r\n\
<FI><ORG>Big Bank<FID>1234</FI></SONRS></SIGNONMSGSRSV1>\r\n\
<BANKMSGSRSV1><STMTTRNRS><TRNUID>1<STMTRS><CURDEF>USD\r\n\
<BANKACCTFROM><BANKID>021000021<ACCTID>000123456789<ACCTTYPE>CHECKING</BANKACCTFROM>\r\n\
<BANKTRANLIST><DTSTART>20260920<DTEND>20261106\r\n\
<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261005120000[-5:EST]<TRNAMT>-4.50<FITID>F1<NAME>SQ *COFFEE SHOP<MEMO>CARD 1111</STMTTRN>\r\n\
<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261006<TRNAMT>-20.00<FITID>F2<NAME>AMAZON MKTPLACE PMTS<REFNUM>9988</STMTTRN>\r\n\
<STMTTRN><TRNTYPE>CHECK<DTPOSTED>20261007<TRNAMT>-80.00<FITID>F3<CHECKNUM>101<NAME>CHECK 101</STMTTRN>\r\n\
<STMTTRN><TRNTYPE>CREDIT<DTPOSTED>20261008<TRNAMT>1,500.00<FITID>F4<NAME>PAYROLL &amp; CO\r\nDIRECT DEP</STMTTRN>\r\n\
<STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20260925<TRNAMT>-9.99<FITID>F5<NAME>OLD THING</STMTTRN>\r\n\
</BANKTRANLIST><LEDGERBAL><BALAMT>1385.51<DTASOF>20261106</LEDGERBAL></STMTRS></STMTTRNRS></BANKMSGSRSV1>\r\n\
<CREDITCARDMSGSRSV1><CCSTMTTRNRS><TRNUID>2<CCSTMTRS><CURDEF>USD<CCACCTFROM><ACCTID>4111222233334444</CCACCTFROM>\r\n\
<BANKTRANLIST><STMTTRN><TRNTYPE>DEBIT<DTPOSTED>20261010<TRNAMT>-12.34<FITID>C1<NAME>GAS</STMTTRN></BANKTRANLIST>\r\n\
</CCSTMTRS></CCSTMTTRNRS></CREDITCARDMSGSRSV1></OFX>\r\n";

const XML: &str = r#"<?xml version="1.0" encoding="UTF-8" standalone="no"?>
<?OFX OFXHEADER="200" VERSION="211" SECURITY="NONE" OLDFILEUID="NONE" NEWFILEUID="NONE"?>
<OFX>
  <SIGNONMSGSRSV1><SONRS><STATUS><CODE>0</CODE><SEVERITY>INFO</SEVERITY></STATUS><DTSERVER>20261106</DTSERVER></SONRS></SIGNONMSGSRSV1>
  <BANKMSGSRSV1><STMTTRNRS><STMTRS>
    <BANKACCTFROM><BANKID>987654321</BANKID><ACCTID>5550001</ACCTID><ACCTTYPE>SAVINGS</ACCTTYPE></BANKACCTFROM>
    <BANKTRANLIST>
      <STMTTRN><TRNTYPE>INT</TRNTYPE><DTPOSTED>20261031</DTPOSTED><TRNAMT>0.42</TRNAMT><FITID>X1</FITID><NAME>Interest</NAME></STMTTRN>
    </BANKTRANLIST>
  </STMTRS></STMTTRNRS></BANKMSGSRSV1>
</OFX>"#;

fn opts(name: &str) -> OpenOptions {
    OpenOptions { backup_on_first_save: false, ..OpenOptions::new(name, "test") }
}

fn category(store: &Store, master: &str, sub: &str) -> String {
    let m = store.collection("masterCategories").into_iter().find(|m| m.str_of("name") == Some(master)).unwrap();
    store.children(m.id(), "subCategories").into_iter().find(|s| s.str_of("name") == Some(sub)).unwrap().id().to_string()
}

fn import_opts(account: &str) -> ImportOptions {
    ImportOptions { account_id: account.into(), import_memos: true, flip_payee_memo: false, include_before_start: false }
}

fn live_in(store: &Store, account: &str) -> usize {
    store.collection("transactions").iter().filter(|t| !t.tomb() && t.str_of("accountId") == Some(account)).count()
}

#[test]
fn parses_sgml_and_xml_ofx_like_ynab() {
    let s = parse_ofx(SGML, 2).unwrap();
    assert_eq!(s.len(), 2);
    let bank = &s[0];
    assert_eq!(bank.kind, "bank");
    // SIGNON's FID replaces the statement's BANKID.
    assert_eq!(bank.fid.as_deref(), Some("1234"));
    assert_eq!(bank.bank_account_id.as_deref(), Some("000123456789"));
    assert_eq!(bank.last4(), "6789");
    assert_eq!(bank.txns.len(), 5);
    let t = &bank.txns;
    assert_eq!((t[0].date.as_str(), t[0].amount, t[0].payee.as_deref(), t[0].memo.as_deref()), ("2026-10-05", -4.5, Some("SQ *COFFEE SHOP"), Some("CARD 1111")));
    assert_eq!(t[0].ynab_id, "YNAB:-4.50:2026-10-05:1");
    // A REFNUM isn't a check number on a DEBIT; CHECKNUM is.
    assert_eq!(t[1].check_number, None);
    assert_eq!(t[2].check_number.as_deref(), Some("101"));
    // Entities decoded; continuation lines joined with a space.
    assert_eq!(t[3].payee.as_deref(), Some("PAYROLL & CO DIRECT DEP"));
    assert_eq!(t[3].ynab_id, "YNAB:1,500.00:2026-10-08:1");
    assert_eq!(s[1].kind, "creditCard");
    assert_eq!(s[1].fid.as_deref(), Some("1234"));
    assert_eq!(s[1].txns[0].amount, -12.34);

    let x = parse_ofx(XML, 2).unwrap();
    assert_eq!(x.len(), 1);
    assert_eq!(x[0].fid.as_deref(), Some("987654321"), "no SIGNON FID: the BANKID is used");
    assert_eq!(x[0].account_type.as_deref(), Some("SAVINGS"));
    assert_eq!((x[0].txns[0].date.as_str(), x[0].txns[0].amount, x[0].txns[0].payee.as_deref()), ("2026-10-31", 0.42, Some("Interest")));

    assert!(parse_ofx("Date,Amount\n1/1/2026,5", 2).is_err());
}

#[test]
fn imports_matches_dedupes_and_applies_rules() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Import", "en_US", opts("Laptop")).unwrap();
    let dining = category(&pkg.store, "Everyday Expenses", "Restaurants");
    let household = category(&pkg.store, "Everyday Expenses", "Household Goods");
    let mut ed = Editor::new(&mut pkg.store, 2);
    let checking = ed
        .create_account(&AccountInput {
            name: "Checking".into(),
            account_type: "Checking".into(),
            on_budget: true,
            starting_balance: 1000.0,
            starting_date: Some("2026-10-01".into()),
            note: None,
        })
        .unwrap();
    // Entered by hand two days before the bank posted it.
    let coffee = ed
        .save_transaction(&TransactionInput {
            account_id: checking.clone(),
            date: "2026-10-03".into(),
            amount: -4.5,
            payee_name: Some("Coffee Shop".into()),
            category_id: Some(dining.clone()),
            memo: Some("latte".into()),
            ..Default::default()
        })
        .unwrap();
    let coffee_payee = ed.store.get(&coffee).unwrap().str_of("payeeId").unwrap().to_string();
    // A payee with a rename rule and an auto-fill category.
    let amazon = ed.get_or_create_payee("Amazon", true).unwrap();
    ed.store.update(&amazon, vec![("autoFillCategoryId", Value::String(household.clone()))]);
    let mut rule = blank_entity("payeeStringCondition", &new_guid());
    rule.insert("operator".into(), Value::String("Contains".into()));
    rule.insert("operand".into(), Value::String("amazon".into()));
    rule.insert("parentPayeeId".into(), Value::String(amazon.clone()));
    ed.store.insert(rule, Some(&amazon)).unwrap();

    let stmt = parse_ofx(SGML, 2).unwrap().remove(0);
    let before = live_in(ed.store, &checking);
    let sum = ed.import_bank_transactions(&stmt.txns, &import_opts(&checking)).unwrap();
    assert_eq!((sum.imported, sum.matched, sum.duplicates, sum.before_start), (3, 1, 0, 1));
    assert_eq!(sum.start_date.as_deref(), Some("2026-10-01"));
    // 3 new; the match replaces the hand-entered one.
    assert_eq!(live_in(ed.store, &checking), before + 3);

    // The match: bank's id, your date / payee / category / memo, cleared, waiting for approval.
    let m_id = "Transaction/YNAB:-4.50:2026-10-05:1";
    let m = ed.store.get(m_id).unwrap().clone();
    assert_eq!(m.str_of("date"), Some("2026-10-03"));
    assert_eq!(m.str_of("payeeId"), Some(coffee_payee.as_str()));
    assert_eq!(m.str_of("categoryId"), Some(dining.as_str()));
    assert_eq!(m.str_of("memo"), Some("latte"));
    assert_eq!(m.str_of("cleared"), Some("Cleared"));
    assert_eq!(m.str_of("source"), Some("Imported"));
    assert_eq!(m.str_of("FITID"), Some("F1"));
    assert_eq!(m.str_of("importedPayee"), Some("SQ *COFFEE SHOP"));
    assert_eq!(m.get("accepted"), Some(&Value::Bool(false)));
    assert!(ed.store.get(&coffee).unwrap().tomb(), "the hand-entered original is replaced");
    let kids = ed.store.children(m_id, "matchedTransactions");
    let kid_ids: Vec<&str> = kids.iter().map(|k| k.id()).collect();
    assert_eq!(kid_ids, vec![format!("MT/{m_id}/MatchedTransaction/{m_id}"), format!("MT/{m_id}/MatchedTransaction/{coffee}")]);
    assert!(kids.iter().all(|k| k.etype() == "matchedTransaction" && k.str_of("parentTransactionIdIfMatched") == Some(m_id)));
    assert_eq!(kids[0].str_of("date"), Some("2026-10-05"));

    // Rename rule + auto-fill category.
    let amz = ed.store.get("Transaction/YNAB:-20.00:2026-10-06:1").unwrap();
    assert_eq!(amz.str_of("payeeId"), Some(amazon.as_str()));
    assert_eq!(amz.str_of("categoryId"), Some(household.as_str()));
    assert_eq!(amz.str_of("importedPayee"), Some("AMAZON MKTPLACE PMTS"));
    let chk = ed.store.get("Transaction/YNAB:-80.00:2026-10-07:1").unwrap();
    assert_eq!(chk.str_of("checkNumber"), Some("101"));
    assert_eq!(chk.str_of("cleared"), Some("Cleared"));
    let pay = ed.store.get("Transaction/YNAB:1,500.00:2026-10-08:1").unwrap();
    assert_eq!(ed.store.get(pay.str_of("payeeId").unwrap()).unwrap().str_of("name"), Some("PAYROLL & CO DIRECT DEP"));
    assert!(ed.store.get("Transaction/YNAB:-9.99:2026-09-25:1").is_none(), "dated before the account started");

    // The same file again: everything is already there.
    let again = ed.import_bank_transactions(&stmt.txns, &import_opts(&checking)).unwrap();
    assert_eq!((again.imported, again.matched, again.duplicates, again.before_start), (0, 0, 4, 1));

    // Approving drops the kept copies but leaves an empty array, as YNAB 4 does.
    ed.approve(&[m_id.to_string()]).unwrap();
    assert!(ed.store.children(m_id, "matchedTransactions").is_empty());
    assert_eq!(ed.store.materialize(m_id).get("matchedTransactions"), Some(&Value::Array(vec![])));
    assert_eq!(ed.store.get(m_id).unwrap().get("accepted"), Some(&Value::Bool(true)));

    // Including the older transaction moves the starting balance so today's balance holds.
    let incl = ImportOptions { include_before_start: true, ..import_opts(&checking) };
    let r = ed.import_bank_transactions(&stmt.txns, &incl).unwrap();
    assert_eq!((r.imported, r.duplicates, r.starting_balance_adjustment), (1, 4, 9.99));
    let sb = ed
        .store
        .collection("transactions")
        .into_iter()
        .find(|t| !t.tomb() && t.str_of("accountId") == Some(checking.as_str()) && ed.store.get(t.str_of("payeeId").unwrap()).unwrap().str_of("name") == Some("Starting Balance"))
        .unwrap();
    assert_eq!((sb.f("amount"), sb.str_of("date")), (1009.99, Some("2026-09-25")));
}

#[test]
fn unmatch_restores_both_and_fitid_fallback_dedupes() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Unmatch", "en_US", opts("Laptop")).unwrap();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let acct = ed
        .create_account(&AccountInput {
            name: "Card".into(),
            account_type: "CreditCard".into(),
            on_budget: true,
            starting_balance: 0.0,
            starting_date: Some("2026-09-01".into()),
            note: None,
        })
        .unwrap();
    let mine = ed
        .save_transaction(&TransactionInput { account_id: acct.clone(), date: "2026-10-09".into(), amount: -12.34, payee_name: Some("Gas Station".into()), ..Default::default() })
        .unwrap();
    let cc = parse_ofx(SGML, 2).unwrap().remove(1);
    let r = ed.import_bank_transactions(&cc.txns, &import_opts(&acct)).unwrap();
    assert_eq!(r.matched, 1);
    let m_id = "Transaction/YNAB:-12.34:2026-10-10:1";
    ed.unmatch(m_id).unwrap();
    let restored = ed.store.get(&mine).unwrap();
    assert!(!restored.tomb());
    assert_eq!(restored.str_of("date"), Some("2026-10-09"));
    let bank = ed.store.get(m_id).unwrap();
    assert_eq!(bank.str_of("date"), Some("2026-10-10"));
    assert_eq!(ed.store.get(bank.str_of("payeeId").unwrap()).unwrap().str_of("name"), Some("GAS"));
    assert_eq!(bank.get("accepted"), Some(&Value::Bool(false)));
    assert!(ed.store.children(m_id, "matchedTransactions").is_empty());
    assert!(ed.unmatch(m_id).is_err());

    // A transaction imported earlier with the same FITID (id from an older YNAB) counts as already
    // imported when the amount matches and the date is within 4 days; it gets the new import id.
    let mut shifted = cc.txns.clone();
    shifted[0].ynab_id = "YNAB:-12.34:2026-10-12:1".into();
    shifted[0].date = "2026-10-12".into();
    let r = ed.import_bank_transactions(&shifted, &import_opts(&acct)).unwrap();
    assert_eq!((r.imported, r.duplicates), (0, 1));
    assert_eq!(ed.store.get(m_id).unwrap().str_of("YNABID"), Some("YNAB:-12.34:2026-10-12:1"));
}

#[test]
fn account_mappings_are_hashed_and_found_again() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Mappings", "en_US", opts("Laptop")).unwrap();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let acct = ed
        .create_account(&AccountInput {
            name: "Checking".into(),
            account_type: "Checking".into(),
            on_budget: true,
            starting_balance: 0.0,
            starting_date: Some("2026-10-01".into()),
            note: None,
        })
        .unwrap();
    assert!(ed.find_account_mapping(Some("1234"), "000123456789").is_none());
    let id = ed.save_account_mapping(Some("1234"), "000123456789", Some(&acct), false, true, false).unwrap();
    let m = ed.store.get(&id).unwrap();
    assert_eq!(m.str_of("shortenedAccountId"), Some("6789"));
    assert_eq!(m.str_of("hash").unwrap().len(), 44);
    assert!(!m.str_of("salt").unwrap().is_empty());
    assert!(!format!("{:?}", ed.store.materialize(&id)).contains("000123456789"), "the account number is only stored hashed");
    // Bank ids are compared trimmed (some saved by YNAB 4 carry a trailing space).
    let found = ed.find_account_mapping(Some("1234 "), "000123456789").unwrap();
    assert_eq!(found.account_id.as_deref(), Some(acct.as_str()));
    assert!(found.import_memos && !found.flip_payee_memo && !found.skip);
    assert!(ed.find_account_mapping(Some("1234"), "000123456780").is_none());
    // Saving again updates rather than duplicates.
    ed.save_account_mapping(Some("1234"), "000123456789", Some(&acct), true, false, false).unwrap();
    assert_eq!(ed.store.collection("accountMappings").iter().filter(|m| !m.tomb()).count(), 1);
    assert!(ed.find_account_mapping(Some("1234"), "000123456789").unwrap().flip_payee_memo);
}

#[test]
fn matches_survive_save_reopen_and_travel_inline_in_diffs() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Sync", "en_US", opts("Laptop")).unwrap();
    let guid = pkg.device.device_guid.clone();
    let acct;
    {
        let mut ed = Editor::new(&mut pkg.store, 2);
        acct = ed
            .create_account(&AccountInput {
                name: "Card".into(),
                account_type: "CreditCard".into(),
                on_budget: true,
                starting_balance: 0.0,
                starting_date: Some("2026-09-01".into()),
                note: None,
            })
            .unwrap();
        ed.save_transaction(&TransactionInput { account_id: acct.clone(), date: "2026-10-10".into(), amount: -12.34, payee_name: Some("Gas".into()), ..Default::default() })
            .unwrap();
        let cc = parse_ofx(SGML, 2).unwrap().remove(1);
        assert_eq!(ed.import_bank_transactions(&cc.txns, &import_opts(&acct)).unwrap().matched, 1);
    }
    let diff = pkg.publish().unwrap().expect("a diff was published");
    let items = read_json(&diff).unwrap();
    let item = items["items"].as_array().unwrap().iter().find(|i| i["entityId"] == "Transaction/YNAB:-12.34:2026-10-10:1").unwrap().clone();
    let kids = item["matchedTransactions"].as_array().unwrap();
    assert_eq!(kids.len(), 2);
    assert!(kids.iter().all(|k| k["entityType"] == "matchedTransaction"));
    assert!(items["items"].as_array().unwrap().iter().all(|i| i["entityType"] != "matchedTransaction"), "kept copies travel inside their parent");
    pkg.save_full(true).unwrap();

    let again = BudgetPackage::open(&pkg.folder, OpenOptions { device_guid: Some(guid), ..opts("Laptop") }).unwrap();
    assert_eq!(again.store.children("Transaction/YNAB:-12.34:2026-10-10:1", "matchedTransactions").len(), 2);
    let text = std::fs::read_to_string(again.full_file_path()).unwrap();
    let root = parse(&text).unwrap();
    let t = root["transactions"].as_array().unwrap().iter().find(|t| t["entityId"] == "Transaction/YNAB:-12.34:2026-10-10:1").unwrap();
    assert_eq!(t["matchedTransactions"].as_array().unwrap().len(), 2);
    let _ = acct;
}

#[test]
fn csv_import_dedupes_on_reimport() {
    let dir = tempfile::tempdir().unwrap();
    let mut pkg = create_budget(dir.path(), "Csv", "en_US", opts("Laptop")).unwrap();
    let mut ed = Editor::new(&mut pkg.store, 2);
    let acct = ed
        .create_account(&AccountInput {
            name: "Checking".into(),
            account_type: "Checking".into(),
            on_budget: true,
            starting_balance: 0.0,
            starting_date: Some("2026-10-01".into()),
            note: None,
        })
        .unwrap();
    let rows = read_csv("Date,Description,Amount\n10/02/2026,Shop,-5.00\n10/02/2026,Shop,-5.00\n10/03/2026,Refund,5.00\n");
    let m = guess_csv_mapping(&rows, "MDY");
    let (txns, problems) = csv_transactions(&rows, &m, 2);
    assert!(problems.is_empty());
    let r = ed.import_bank_transactions(&txns, &import_opts(&acct)).unwrap();
    assert_eq!(r.imported, 3, "two identical rows are two transactions (…:1 and …:2)");
    let r = ed.import_bank_transactions(&txns, &import_opts(&acct)).unwrap();
    assert_eq!((r.imported, r.duplicates), (0, 3));
}
