import { useEffect, useState } from 'react';
import { api } from '../api.ts';

/** Shown at startup when a PIN is set; nothing else loads until it's entered. */
export function LockScreen({ onUnlocked }: { onUnlocked: () => void }) {
  const [pin, setPin] = useState('');
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const submit = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.unlock(pin);
      onUnlocked();
    } catch (e) {
      setError((e as Error).message);
      setPin('');
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="welcome">
      <form
        className="welcome-card lock-card"
        onSubmit={(e) => {
          e.preventDefault();
          if (pin) void submit();
        }}
      >
        <h1>
          <img src="/favicon.svg" alt="" width={32} height={32} /> Budget4
        </h1>
        <label>
          Enter your PIN
          <input type="password" autoFocus value={pin} onChange={(e) => setPin(e.target.value)} disabled={busy} autoComplete="off" />
        </label>
        <div className="row end">
          <button className="primary" disabled={busy || !pin}>
            {busy ? 'Checking…' : 'Unlock'}
          </button>
        </div>
        {error && <p className="error">{error}</p>}
      </form>
    </div>
  );
}

/** Settings section: turn the PIN on, change it, or remove it. */
export function PinSettings({ setToast }: { setToast: (t: string) => void }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [mode, setMode] = useState<'set' | 'remove' | null>(null);
  const [current, setCurrent] = useState('');
  const [pin, setPin] = useState('');
  const [confirm, setConfirm] = useState('');
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .lockStatus()
      .then((s) => setEnabled(s.enabled))
      .catch(() => {});
  }, []);
  if (enabled === null) return null;

  const reset = () => {
    setMode(null);
    setCurrent('');
    setPin('');
    setConfirm('');
    setError(null);
  };

  const submit = async () => {
    setError(null);
    try {
      if (mode === 'set') {
        if (pin !== confirm) return setError("The two PINs don't match");
        await api.setPin(pin, enabled ? current : undefined);
        setToast(enabled ? 'PIN changed' : 'PIN turned on. Budget4 will ask for it when it starts.');
        setEnabled(true);
      } else {
        await api.removePin(current);
        setToast('PIN removed');
        setEnabled(false);
      }
      reset();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <section>
      <h3>PIN</h3>
      <p>
        {enabled
          ? 'Budget4 asks for a PIN every time it starts.'
          : 'Require a PIN (or password) every time Budget4 starts.'}{' '}
        <span className="muted small">
          This locks the app only: the budget files stay in YNAB 4's format, readable by anyone with access to the folder. There's no
          way to recover a forgotten PIN; to reset it, delete the "pinHash" line from .budget4\config.json in your user folder.
        </span>
      </p>
      {mode === null ? (
        <div className="row">
          <button onClick={() => setMode('set')}>{enabled ? 'Change PIN' : 'Set a PIN'}</button>
          {enabled && <button onClick={() => setMode('remove')}>Remove PIN</button>}
        </div>
      ) : (
        <form
          className="pin-form"
          onSubmit={(e) => {
            e.preventDefault();
            void submit();
          }}
        >
          {enabled && (
            <label>
              Current PIN
              <input type="password" autoFocus value={current} onChange={(e) => setCurrent(e.target.value)} autoComplete="off" />
            </label>
          )}
          {mode === 'set' && (
            <>
              <label>
                New PIN (at least 4 characters)
                <input type="password" autoFocus={!enabled} value={pin} onChange={(e) => setPin(e.target.value)} autoComplete="off" />
              </label>
              <label>
                New PIN again
                <input type="password" value={confirm} onChange={(e) => setConfirm(e.target.value)} autoComplete="off" />
              </label>
            </>
          )}
          <div className="row">
            <button type="button" onClick={reset}>
              Cancel
            </button>
            <button className="primary" disabled={mode === 'set' ? !pin || !confirm || (enabled && !current) : !current}>
              {mode === 'set' ? 'Save PIN' : 'Remove PIN'}
            </button>
          </div>
          {error && <p className="error">{error}</p>}
        </form>
      )}
    </section>
  );
}
