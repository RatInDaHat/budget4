import { useEffect, useMemo, useRef, useState } from 'react';

export interface Option {
  id: string;
  label: string;
  group?: string;
  hint?: string;
}

const NEW = '__new__';

function rank(o: Option, q: string): number {
  const l = o.label.toLowerCase();
  if (l === q) return 0;
  if (l.startsWith(q)) return 1;
  // Start of a later word ("mart" in "Wal Mart", "fuel" in "Transfer : Fuel card").
  if (l.split(/[\s:/()-]+/).some((w) => w.startsWith(q))) return 2;
  if (o.group && o.group.toLowerCase().startsWith(q)) return 3;
  if (l.includes(q)) return 4;
  return -1;
}

/**
 * Text input with a filtered dropdown.
 *
 * With `allowNew` (payees) the typed text becomes a new entry unless the user explicitly picks a
 * suggestion, or the text starts an existing name (then Enter/Tab completes it, as in YNAB).
 */
export function Autocomplete({
  value,
  text,
  options,
  onChange,
  placeholder,
  allowNew,
  disabled,
  autoFocus,
  onEnter,
  ariaLabel,
}: {
  value: string | null;
  text?: string;
  options: Option[];
  onChange: (id: string | null, text: string) => void;
  placeholder?: string;
  allowNew?: boolean;
  disabled?: boolean;
  autoFocus?: boolean;
  onEnter?: () => void;
  ariaLabel?: string;
}) {
  const selected = value ? options.find((o) => o.id === value) : undefined;
  const display = selected?.label ?? text ?? '';
  const [query, setQuery] = useState(display);
  const [open, setOpen] = useState(false);
  const [typed, setTyped] = useState(false);
  const [hi, setHi] = useState(0);
  const listRef = useRef<HTMLUListElement>(null);
  // Mirrors of state for event handlers that run before React re-renders (blur right after Tab).
  const live = useRef({ query, open, typed });
  live.current = { query, open, typed };
  const committed = useRef(false);

  useEffect(() => {
    if (!open) setQuery(display);
  }, [display, open]);

  const q = query.trim().toLowerCase();
  const filtered = useMemo<Option[]>(() => {
    if (!typed || !q) return options.slice(0, 300);
    const scored: [number, Option][] = [];
    for (const o of options) {
      const r = rank(o, q);
      if (r >= 0) scored.push([r, o]);
    }
    scored.sort((a, b) => a[0] - b[0] || a[1].label.localeCompare(b[1].label));
    const out = scored.slice(0, 300).map((s) => s[1]);
    if (allowNew && !out.some((o) => o.label.toLowerCase() === q)) {
      const createNew = { id: NEW, label: `Create payee "${query.trim()}"` };
      // Offer the new name first unless an existing name starts with what was typed.
      if (out.length && rank(out[0], q) <= 1) out.splice(1, 0, createNew);
      else out.unshift(createNew);
    }
    return out;
  }, [options, q, typed, allowNew, query]);

  useEffect(() => setHi(0), [q]);
  useEffect(() => {
    listRef.current?.children[hi]?.scrollIntoView({ block: 'nearest' });
  }, [hi]);

  const commit = (o: Option | undefined) => {
    committed.current = true;
    setOpen(false);
    setTyped(false);
    if (o && o.id !== NEW) {
      setQuery(o.label);
      onChange(o.id, o.label);
    } else if (allowNew && live.current.query.trim()) {
      const t = live.current.query.trim();
      const exact = options.find((x) => x.label.toLowerCase() === t.toLowerCase());
      if (exact) {
        setQuery(exact.label);
        onChange(exact.id, exact.label);
      } else {
        setQuery(t);
        onChange(null, t);
      }
    } else if (allowNew) {
      setQuery('');
      onChange(null, '');
    } else {
      setQuery(display);
    }
  };

  /** What Enter/Tab should choose. */
  const choice = (): Option | undefined => {
    if (!live.current.typed) return selected;
    return filtered[hi]; // may be the "Create payee" entry
  };

  return (
    <div className="ac">
      <input
        value={query}
        placeholder={placeholder}
        disabled={disabled}
        autoFocus={autoFocus}
        aria-label={ariaLabel}
        onFocus={(e) => {
          committed.current = false;
          setOpen(true);
          setTyped(false);
          e.target.select();
        }}
        onChange={(e) => {
          setQuery(e.target.value);
          setOpen(true);
          setTyped(true);
        }}
        onBlur={() => {
          if (committed.current) return;
          if (!live.current.typed) {
            setOpen(false);
            return;
          }
          // Typed and left without choosing: exact match wins, otherwise a new payee / revert.
          const t = live.current.query.trim().toLowerCase();
          const exact = options.find((o) => o.label.toLowerCase() === t);
          commit(exact ?? (!allowNew && filtered.length === 1 ? filtered[0] : undefined));
        }}
        onKeyDown={(e) => {
          if (e.key === 'ArrowDown') {
            e.preventDefault();
            setOpen(true);
            setHi((h) => Math.min(h + 1, filtered.length - 1));
          } else if (e.key === 'ArrowUp') {
            e.preventDefault();
            setHi((h) => Math.max(h - 1, 0));
          } else if (e.key === 'Enter') {
            e.preventDefault();
            if (live.current.typed) commit(choice());
            else onEnter?.();
          } else if (e.key === 'Tab') {
            if (live.current.typed) commit(choice());
          } else if (e.key === 'Escape') {
            committed.current = true;
            setOpen(false);
            setTyped(false);
            setQuery(display);
          }
        }}
      />
      {open && !disabled && filtered.length > 0 && (
        <ul className="ac-list" ref={listRef} role="listbox">
          {filtered.map((o, i) => (
            <li
              key={o.id}
              role="option"
              aria-selected={i === hi}
              className={`${i === hi ? 'hi' : ''} ${o.id === NEW ? 'ac-new' : ''}`}
              onMouseDown={(e) => {
                e.preventDefault();
                commit(o);
              }}
              onMouseEnter={() => setHi(i)}
            >
              {o.group && <span className="ac-group">{o.group}</span>}
              <span>{o.label}</span>
              {o.hint && <span className="ac-hint">{o.hint}</span>}
            </li>
          ))}
        </ul>
      )}
    </div>
  );
}
