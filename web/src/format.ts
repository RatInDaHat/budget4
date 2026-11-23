let currencyFmt = new Intl.NumberFormat('en-US', { style: 'currency', currency: 'USD' });
let plainFmt = new Intl.NumberFormat('en-US', { minimumFractionDigits: 2, maximumFractionDigits: 2 });
let digits = 2;
let localeTag = 'en-US';

export function configureFormat(locale: string, currency: string, decimalDigits: number) {
  localeTag = locale;
  digits = decimalDigits;
  try {
    currencyFmt = new Intl.NumberFormat(locale, { style: 'currency', currency, minimumFractionDigits: decimalDigits, maximumFractionDigits: decimalDigits });
    plainFmt = new Intl.NumberFormat(locale, { minimumFractionDigits: decimalDigits, maximumFractionDigits: decimalDigits });
  } catch {
    /* keep defaults */
  }
}

export const money = (n: number) => currencyFmt.format(Object.is(n, -0) ? 0 : n);
export const plain = (n: number) => plainFmt.format(Object.is(n, -0) ? 0 : n);
export const round = (n: number) => Math.round(n * 10 ** digits) / 10 ** digits;

/** Parses user input like "1,234.50", "-12", "$5", "(4.00)", or a simple sum "10+2.5". */
export function parseAmount(input: string): number | null {
  const s = input.trim();
  if (!s) return 0;
  const negParen = /^\(.*\)$/.test(s);
  const cleaned = s.replace(/[()\s]/g, '').replace(/[^\d.,+\-*/]/g, '');
  // Treat "," as thousands separator unless it is clearly the decimal separator ("12,50").
  const normalized = /,\d{1,2}$/.test(cleaned) && !cleaned.includes('.') ? cleaned.replace(/\./g, '').replace(',', '.') : cleaned.replace(/,/g, '');
  if (!/^[-+]?[\d.]+([-+*/][\d.]+)*$/.test(normalized)) return null;
  const v = evaluate(normalized);
  if (v === null || !Number.isFinite(v)) return null;
  return round(negParen ? -Math.abs(v) : v);
}

/** Evaluates "a+b-c*d/e" with the usual precedence. */
function evaluate(expr: string): number | null {
  const tokens = expr.match(/[\d.]+|[-+*/]/g) ?? [];
  let i = 0;
  const num = (): number | null => {
    let sign = 1;
    while (tokens[i] === '-' || tokens[i] === '+') sign *= tokens[i++] === '-' ? -1 : 1;
    const t = tokens[i++];
    if (t === undefined || !/^\d*\.?\d*$/.test(t) || t === '.') return null;
    return sign * Number(t);
  };
  const term = (): number | null => {
    let v = num();
    while (v !== null && (tokens[i] === '*' || tokens[i] === '/')) {
      const op = tokens[i++];
      const r = num();
      if (r === null) return null;
      v = op === '*' ? v * r : v / r;
    }
    return v;
  };
  let v = term();
  while (v !== null && (tokens[i] === '+' || tokens[i] === '-')) {
    const op = tokens[i++];
    const r = term();
    if (r === null) return null;
    v = op === '+' ? v + r : v - r;
  }
  return i === tokens.length ? v : null;
}

const MONTHS = ['January', 'February', 'March', 'April', 'May', 'June', 'July', 'August', 'September', 'October', 'November', 'December'];
const SHORT = ['Jan', 'Feb', 'Mar', 'Apr', 'May', 'Jun', 'Jul', 'Aug', 'Sep', 'Oct', 'Nov', 'Dec'];

export const monthName = (m: string) => `${MONTHS[Number(m.slice(5, 7)) - 1]} ${m.slice(0, 4)}`;
export const shortMonth = (m: string) => SHORT[Number(m.slice(5, 7)) - 1];

export function addMonths(month: string, n: number): string {
  const [y, m] = month.split('-').map(Number);
  const idx = y * 12 + (m - 1) + n;
  return `${Math.floor(idx / 12)}-${String((idx % 12) + 1).padStart(2, '0')}`;
}

export function today(): string {
  const d = new Date();
  return `${d.getFullYear()}-${String(d.getMonth() + 1).padStart(2, '0')}-${String(d.getDate()).padStart(2, '0')}`;
}

export function formatDate(iso: string): string {
  const [y, m, d] = iso.split('-').map(Number);
  return new Date(y, m - 1, d).toLocaleDateString(localeTag, { year: 'numeric', month: '2-digit', day: '2-digit' });
}

export function relativeTime(ms: number | null): string {
  if (!ms) return 'never';
  const s = Math.round((Date.now() - ms) / 1000);
  if (s < 10) return 'just now';
  if (s < 60) return `${s}s ago`;
  if (s < 3600) return `${Math.round(s / 60)}m ago`;
  return new Date(ms).toLocaleTimeString(localeTag, { hour: 'numeric', minute: '2-digit' });
}
