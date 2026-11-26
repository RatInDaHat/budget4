# Budget4

A desktop budgeting app that opens **YNAB 4** budgets and keeps working with them. It reads and
writes the YNAB 4 file format and syncs through Dropbox as one more YNAB 4 device, so YNAB 4 on your
other computers sees its changes and it sees theirs, within seconds.

Written in Rust (the format, sync engine and budget math) with a React UI in a native window (Tauri).

## Why this exists

YNAB 4 is no longer supported, but I have used it for years, and my budget files still work. The
newer YNAB is a cloud service, and for several reasons I did not want my money data in the cloud.
Rather than move to it, I built Budget4 to keep working with my budget in its own folder, so the
data never leaves the computer or the Dropbox folder it already lived in.

> **Back up your budget first.** Budget4 writes into a live YNAB 4 budget. It has been checked
> carefully against real budgets (see [Safety](#safety)), but a bug could still damage your data.
> Try it on a copy of your budget folder before using it on the real one. Use at your own risk.

* Budget screen: months with Not budgeted / Overspent / Income / Budgeted / Available to Budget, category
  balances with rollover, overspending handling ("keep it in the category"), notes, quick-budget actions,
  category groups (add, rename, hide/unhide, delete with reassignment).
* Accounts: budget and off-budget accounts with cleared / uncleared / working balances, a fast register
  (search, multi-select, cleared toggling, approval of auto-entered transactions, editing in place, and
  an entry row pinned under the headings for adding transactions in place), transfers, splits
  (including transfer lines), reconciliation with adjustment, closing accounts.
* Scheduled transactions: entered automatically on their date as YNAB 4 does, plus "Enter now" and "Skip".
* Importing bank files: OFX, QFX, QBO and CSV (any column layout). Like YNAB 4 it skips transactions
  imported before, matches the rest to ones you entered (approve or unmatch them in the register),
  applies payee rename rules and auto-fill categories, and remembers which bank account goes where in
  the budget itself, so YNAB 4's saved choices are reused and vice versa. Drop a file onto the budget
  window, or double-click a QFX, OFX or QBO file (Budget4 registers for them on install), to import it.
* Registers: click a column header to sort (again to reverse), drag header edges to resize; each
  account shows its scheduled transactions in a collapsible panel at the bottom. The Scheduled
  page sorts by any column too.
* Payees: rename one, or merge several into one (every transaction moves to the payee you keep).
* Credit cards: each budgeted credit card gets a "Pre-YNAB Debt" category, as in YNAB 4. A zero or
  negative starting balance is budgeted there to pay off; a positive one is income.
* Undo / redo (Ctrl+Z, Ctrl+Y) for every edit this session, imports included. An undo is saved as a
  new change, so it syncs to YNAB 4 and other devices; it refuses if the record was changed since.
* New budgets in YNAB 4's own layout (YNAB 4 can open them).
* Budget list: "Remove from list" takes a budget off the load screen without touching its folder; opening it
  again puts it back.
* Archives: Settings → "Archive budget…" writes the whole budget folder into one zip (saving the full file
  first), and "Restore from an archive…" on the load screen unpacks it to a folder you choose and opens it.

How the format works and what was verified: [docs/ynab4-format.md](docs/ynab4-format.md).

## Install

Run `target\release\bundle\nsis\Budget4_<version>_x64-setup.exe` (build it with `npm run app`). It
installs Budget4 with a Start menu entry. Or double-click **Budget4.bat**, which builds the app the
first time and starts it; drag a `.ynab4` folder onto it to open that budget directly.

Building needs [Rust](https://rustup.rs) and [Node.js](https://nodejs.org) 20+. Windows 10/11 already
have the WebView2 runtime the window uses.

```sh
npm install
npm run app        # release build + installer
npm run dev        # development: hot-reloading UI in the app window
npm test           # Rust tests (cargo test --release)
```

## Safety

* Budget4 joins the budget as its **own device**. It only writes its own device folder, its own
  `.ydevice` file, and `.y4backup` files; it never edits another device's files or `Budget.ymeta`.
* Before the first save of each session it writes a `.y4backup` (YNAB 4's own zip format, so YNAB 4's
  "Restore from backup" can use it).
* Its device file refuses to go backwards (knowledge never regresses) or to overwrite a device file that
  now belongs to another device.
* Changes are published every few seconds and the full file is saved on close.
* Settings live in `~/.budget4/config.json` (device GUID, device name, recent budgets).
* Optional PIN (Settings → PIN): asked for every time Budget4 starts, before any budget is listed or
  opened. It locks the app only; the budget files stay in YNAB 4's unencrypted format so YNAB 4 can
  still read them. It's stored as an Argon2 hash, and wrong tries are slowed down after five. When a
  PIN is set or changed, Budget4 shows a recovery code once; entering it on the lock screen turns the PIN
  off, and you set a new one afterwards. Without a code (a PIN set before this), delete the `pinHash`
  line from `config.json` to reset it.

Try it on a copy of your budget folder first.

## Tests

`npm test` runs the Rust tests. Unit and end-to-end tests use throwaway budgets. Tests that need real
data read a folder of `*.ynab4` packages from `BUDGET4_TESTDATA` (default: a `ynab4-testdata` folder
next to the repository) and are skipped without it. **Point it at a copy**, never a live budget. With a YNAB 4
`… as of YYYY-MM-DD …-Budget.csv` export next to the budget, the budget math is checked against it.

## Not done yet

* QIF import, with drag-and-drop and double-click like the other bank files; exports (QIF/CSV).
* Reports; payee rename rules and locations
  (kept and synced, not editable).
* A separate PIN per budget (today there's one PIN for the app).
* Per-machine UI settings (`budgetSettings.ybsettings`) and the legacy YNAB 3/Pro formats.

## Layout

```
crates/budget4-core/   format, version vectors, entity store, sync engine, budget math, editing
  tests/               real-data and end-to-end tests
src-tauri/             desktop app: commands, sync loop, settings
web/                   React UI shown in the app window
docs/                  format notes
```

## License and trademarks

MIT; see [LICENSE](LICENSE).

Budget4 is an independent project. It is not affiliated with, endorsed by, or supported by
You Need A Budget LLC. "YNAB" and "You Need A Budget" are trademarks of You Need A Budget LLC and
are used here only to say which files Budget4 works with. Budget4 contains no YNAB code.
