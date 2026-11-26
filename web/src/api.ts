import { invoke } from '@tauri-apps/api/core';
import { open as openDialog, save as saveDialog } from '@tauri-apps/plugin-dialog';

export type Cleared = 'Uncleared' | 'Cleared' | 'Reconciled';
export type Handling = 'AffectsBuffer' | 'Confined';

export interface Balances {
  cleared: number;
  uncleared: number;
  reconciled: number;
  working: number;
}

export interface AccountDTO {
  id: string;
  name: string;
  type: string;
  onBudget: boolean;
  hidden: boolean;
  note: string | null;
  lastReconciledDate: string | null;
  lastReconciledBalance: number;
  balances: Balances;
  unapproved: number;
}

export interface SubDTO {
  id: string;
  name: string;
  note: string | null;
}

export interface MasterDTO {
  id: string;
  name: string;
  type: 'OUTFLOW' | 'INFLOW';
  builtIn: boolean;
  note: string | null;
  subs: SubDTO[];
}

export interface PayeeDTO {
  id: string;
  name: string;
  transferAccountId: string | null;
  autoFillCategoryId: string | null;
  autoFillMemo: string | null;
  autoFillAmount: number;
  enabled: boolean;
}

export interface SyncStatus {
  lastSync: number | null;
  lastFullSave: number | null;
  lastImport: number | null;
  unpublished: boolean;
  unsavedFullFile: boolean;
  blockedDiffs: number;
  lastError: string | null;
  conflictsResolved: number;
}

export interface StateDTO {
  budget: {
    name: string;
    folder: string;
    device: { id: string; name: string };
    otherDevices: { id: string; name: string; version: string; knowledge: string | null }[];
    currency: string;
    locale: string;
    decimalDigits: number;
  };
  accounts: AccountDTO[];
  masters: MasterDTO[];
  payees: PayeeDTO[];
  months: { first: string; last: string; current: string };
  sync: SyncStatus;
  /** What Undo / Redo would undo or redo, if anything. */
  undo: { undo: string | null; redo: string | null };
}

export interface CategoryMonthDTO {
  budgeted: number;
  spent: number;
  balance: number;
  handling: Handling;
  handlingSet: Handling | null;
  note: string | null;
}

export interface MonthDTO {
  month: string;
  notBudgetedLastMonth: number;
  overspentLastMonth: number;
  income: number;
  totalBudgeted: number;
  available: number;
  endingBuffer: number;
  supplementalIncome: number;
  deferredIncomeFromThisMonth: number;
  deferredIncomeFromPreviousMonth: number;
  totalCarriedOverspending: number;
  totalBalance: number;
  totalSpent: number;
  note: string | null;
  categories: Record<string, CategoryMonthDTO>;
  masters: Record<string, { budgeted: number; spent: number; balance: number }>;
}

export interface SplitDTO {
  id?: string;
  amount: number;
  categoryId: string | null;
  memo: string | null;
  targetAccountId: string | null;
}

export interface TxnDTO {
  id: string;
  accountId: string;
  date: string;
  amount: number;
  payeeId: string | null;
  categoryId: string | null;
  memo: string | null;
  cleared: Cleared;
  accepted: boolean;
  flag: string | null;
  checkNumber: string | null;
  targetAccountId: string | null;
  transferTransactionId: string | null;
  importedPayee: string | null;
  source: string | null;
  /** Set while an imported transaction is matched to one you entered and not yet approved. */
  matched: { date: string; payeeId: string | null; memo: string | null; bankDate: string | null; bankPayee: string | null } | null;
  isSplit: boolean;
  splits: SplitDTO[];
  runningBalance: number | null;
}

export interface ScheduledDTO extends TxnDTO {
  frequency: string;
  twiceAMonthStartDay: number;
}

export interface AppInfo {
  version: string;
  open: string | null;
  recent: string[];
  budgetFolders: string[];
  friendlyName: string;
  enterScheduledTransactions: boolean;
  backupOnFirstSave: boolean;
}

export interface BudgetListing {
  name: string;
  folder: string;
  modified: number;
}

export interface TransactionInput {
  id?: string;
  accountId: string;
  date: string;
  amount: number;
  payeeId?: string | null;
  payeeName?: string | null;
  transferAccountId?: string | null;
  categoryId?: string | null;
  memo?: string | null;
  cleared?: Cleared;
  flag?: string | null;
  checkNumber?: string | null;
  accepted?: boolean;
  splits?: { id?: string; amount: number; categoryId?: string | null; memo?: string | null; transferAccountId?: string | null }[] | null;
}

export type DateOrder = 'MDY' | 'DMY' | 'YMD';

export interface CsvMapping {
  hasHeader: boolean;
  date: number | null;
  payee: number | null;
  memo: number | null;
  amount: number | null;
  outflow: number | null;
  inflow: number | null;
  checkNumber: number | null;
  dateOrder: DateOrder;
  negate: boolean;
}

export interface AccountMapping {
  id: string;
  accountId: string | null;
  flipPayeeMemo: boolean;
  importMemos: boolean;
  skip: boolean;
}

export interface OfxStatement {
  index: number;
  kind: 'bank' | 'creditCard' | 'investment';
  accountType: string | null;
  last4: string;
  count: number;
  from: string | null;
  to: string | null;
  mapping: AccountMapping | null;
}

export type LoadedFile =
  | { kind: 'ofx'; name: string; statements: OfxStatement[] }
  | { kind: 'csv'; name: string; columns: number; rowCount: number; sample: string[][]; mapping: CsvMapping; dateOrders: DateOrder[] };

export interface ImportRow {
  status: 'new' | 'matched' | 'duplicate' | 'beforeStart';
  date: string;
  amount: number;
  payee: string | null;
  importedPayee: string | null;
  memo: string | null;
  categoryId: string | null;
  matched: { id: string; date: string; payeeId: string | null; memo: string | null } | null;
}

export interface ImportSummary {
  imported: number;
  matched: number;
  duplicates: number;
  beforeStart: number;
  startDate: string | null;
  startingBalanceAdjustment: number;
}

export interface ImportPreview extends ImportSummary {
  rows: ImportRow[];
  problems: string[];
  dateOrders?: DateOrder[];
}

export interface ImportRequest {
  statement?: number;
  mapping?: CsvMapping;
  accountId: string;
  importMemos: boolean;
  flipPayeeMemo: boolean;
  includeBeforeStart: boolean;
  skip?: boolean;
}

/** Every request goes to the Rust side's single `api` command, which routes it like an HTTP API. */
async function call<T>(method: string, path: string, body?: unknown): Promise<T> {
  try {
    return await invoke<T>('api', { method, path, body: body ?? null });
  } catch (e) {
    throw new Error(typeof e === 'string' ? e : (e as Error)?.message ?? String(e));
  }
}

/** Opens the system folder picker. */
export async function pickFolder(title: string): Promise<string | null> {
  const r = await openDialog({ directory: true, multiple: false, title });
  return typeof r === 'string' ? r : null;
}

/** Opens the system file picker for a bank export. */
export async function pickBankFile(): Promise<string | null> {
  const r = await openDialog({
    directory: false,
    multiple: false,
    title: 'Import a bank file',
    filters: [
      { name: 'Bank files (OFX, QFX, QBO, CSV)', extensions: ['ofx', 'qfx', 'qbo', 'csv', 'OFX', 'QFX', 'QBO', 'CSV'] },
      { name: 'All files', extensions: ['*'] },
    ],
  });
  return typeof r === 'string' ? r : null;
}

/** A file's contents as base64, for sending to the app. */
export async function fileBase64(f: File): Promise<string> {
  const bytes = new Uint8Array(await f.arrayBuffer());
  let binary = '';
  for (let i = 0; i < bytes.length; i += 0x8000) binary += String.fromCharCode(...bytes.subarray(i, i + 0x8000));
  return btoa(binary);
}

/** Asks where to save a budget archive (a single .zip). */
export async function pickArchiveSave(defaultName: string): Promise<string | null> {
  const r = await saveDialog({
    title: 'Archive budget',
    defaultPath: defaultName,
    filters: [{ name: 'Budget archive', extensions: ['zip'] }],
  });
  return typeof r === "string" ? r : null;
}

/** Asks for a budget archive made by Archive budget. */
export async function pickArchiveFile(): Promise<string | null> {
  const r = await openDialog({
    directory: false,
    multiple: false,
    title: 'Restore a budget archive',
    filters: [{ name: 'Budget archive', extensions: ['zip'] }],
  });
  return typeof r === "string" ? r : null;
}

export const api = {
  lockStatus: () => call<{ enabled: boolean; locked: boolean; hasRecovery: boolean }>('GET', '/api/lock'),
  unlock: (pin: string) => call('POST', '/api/lock/unlock', { pin }),
  recover: (code: string) => call<{ pinRemoved: boolean }>('POST', '/api/lock/recover', { code }),
  setPin: (pin: string, current?: string) => call<{ recoveryCode: string }>('POST', '/api/lock/set', { pin, current }),
  removePin: (current: string) => call('POST', '/api/lock/remove', { current }),
  app: () => call<AppInfo>('GET', '/api/app'),
  budgets: () => call<BudgetListing[]>('GET', '/api/budgets'),
  /** Takes a budget off the load screen's list. The budget folder is not touched. */
  forgetBudget: (folder: string) => call('POST', '/api/budgets/forget', { folder }),
  open: (folder: string) => call<StateDTO>('POST', '/api/open', { folder }),
  create: (parent: string, name: string, currencyLocale: string, starter: 'personal' | 'business' | 'none') =>
    call<StateDTO>('POST', '/api/create', { parent, name, currencyLocale, starter }),
  close: () => call('POST', '/api/close'),
  settings: (s: Partial<Pick<AppInfo, 'friendlyName' | 'enterScheduledTransactions' | 'backupOnFirstSave'>>) => call('POST', '/api/settings', s),
  state: () => call<StateDTO>('GET', '/api/state'),
  months: (from: string, count: number) => call<MonthDTO[]>('GET', `/api/months/${from}?count=${count}`),
  transactions: (accountId: string | null) => call<TxnDTO[]>('GET', `/api/transactions${accountId ? `?account=${encodeURIComponent(accountId)}` : ''}`),
  scheduled: () => call<ScheduledDTO[]>('GET', '/api/scheduled'),
  log: () => call<string[]>('GET', '/api/log'),
  sync: () => call<{ imported: number; blocked: number; conflicts: number; published: boolean }>('POST', '/api/sync'),
  save: () => call('POST', '/api/save'),
  backup: () => call<{ file: string }>('POST', '/api/backup'),
  /** Zips the open budget folder to a file outside it. Saves the full file first. */
  archive: (file: string) => call<{ file: string }>('POST', '/api/archive', { file }),
  /** Unzips an archive into a parent folder and opens the budget. */
  restoreArchive: (file: string, parent: string) => call<StateDTO>('POST', '/api/archive/restore', { file, parent }),

  saveTransaction: (t: TransactionInput) => call<string>('POST', '/api/transactions', t),
  deleteTransactions: (ids: string[]) => call('POST', '/api/transactions/delete', { ids }),
  toggleCleared: (ids: string[]) => call('POST', '/api/transactions/cleared', { ids }),
  setCleared: (ids: string[], cleared: Cleared) => call('POST', '/api/transactions/cleared', { ids, cleared }),
  approve: (ids: string[]) => call('POST', '/api/transactions/approve', { ids }),
  unmatch: (id: string) => call('POST', '/api/transactions/unmatch', { id }),
  undo: () => call<{ label: string }>('POST', '/api/undo'),
  redo: () => call<{ label: string }>('POST', '/api/redo'),

  importLoad: (path: string, accountId: string | null) => call<LoadedFile>('POST', '/api/import/load', { path, accountId }),
  importUpload: (name: string, data: string, accountId: string | null) => call<LoadedFile>('POST', '/api/import/upload', { name, data, accountId }),
  /** Bank files handed to the app (double-clicked or dropped with no budget open yet). Taken once: the list is then empty. */
  incoming: () => call<string[]>('GET', '/api/incoming'),
  importPreview: (r: ImportRequest) => call<ImportPreview>('POST', '/api/import/preview', r),
  importCommit: (r: ImportRequest) => call<ImportSummary & { problems: number; remembered: boolean; skipped?: boolean }>('POST', '/api/import/commit', r),
  importCancel: () => call('POST', '/api/import/cancel'),

  createAccount: (a: { name: string; type: string; onBudget: boolean; startingBalance: number; startingDate: string; note?: string | null }) =>
    call<string>('POST', '/api/accounts', a),
  updateAccount: (id: string, f: { name?: string; hidden?: boolean; note?: string | null }) => call('PATCH', `/api/accounts/${id}`, f),
  moveAccount: (id: string, beforeId: string | null) => call('POST', `/api/accounts/${id}/move`, { beforeId }),
  reconcile: (id: string, balance: number, date?: string) => call<{ adjustment: number }>('POST', `/api/accounts/${id}/reconcile`, { balance, date }),
  deleteAccount: (id: string) => call('POST', `/api/accounts/${id}/delete`),

  setBudgeted: (month: string, categoryId: string, budgeted: number) => call('POST', '/api/budget', { month, categoryId, budgeted }),
  setHandling: (month: string, categoryId: string, handling: Handling | null) => call('POST', '/api/budget/overspending', { month, categoryId, handling }),
  setBudgetNote: (month: string, categoryId: string | undefined, note: string | null) => call('POST', '/api/budget/note', { month, categoryId, note }),

  createMaster: (name: string) => call<string>('POST', '/api/categories/master', { name }),
  createCategory: (masterId: string, name: string) => call<string>('POST', '/api/categories', { masterId, name }),
  updateCategory: (id: string, f: { name?: string; note?: string | null }) => call('PATCH', `/api/categories/${encodeURIComponent(id)}`, f),
  hideCategory: (id: string) => call('POST', `/api/categories/${encodeURIComponent(id)}/hide`),
  unhideCategory: (id: string) => call('POST', `/api/categories/${encodeURIComponent(id)}/unhide`),
  moveCategory: (id: string, masterId: string | undefined, beforeId: string | null) => call('POST', `/api/categories/${encodeURIComponent(id)}/move`, { masterId, beforeId }),
  deleteCategory: (id: string, reassignTo: string | null) => call('POST', `/api/categories/${encodeURIComponent(id)}/delete`, { reassignTo }),
  deleteMaster: (id: string) => call('POST', `/api/categories/${encodeURIComponent(id)}/delete`, { master: true }),

  renamePayee: (id: string, name: string) => call('PATCH', `/api/payees/${encodeURIComponent(id)}`, { name }),
  mergePayees: (fromIds: string[], intoId: string) => call('POST', '/api/payees/merge', { fromIds, intoId }),

  saveScheduled: (s: TransactionInput & { frequency: string; twiceAMonthStartDay?: number }) => call<string>('POST', '/api/scheduled', s),
  deleteScheduled: (id: string) => call('POST', `/api/scheduled/${encodeURIComponent(id)}/delete`),
  enterScheduled: (id: string) => call('POST', `/api/scheduled/${encodeURIComponent(id)}/enter`),
  skipScheduled: (id: string) => call('POST', `/api/scheduled/${encodeURIComponent(id)}/skip`),
};
