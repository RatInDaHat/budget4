import { useEffect, useState } from 'react';
import { api } from '../api.ts';

/** Shown at startup when a PIN is set; nothing else loads until it's entered or reset. */
export function LockScreen({ onUnlocked }: { onUnlocked: (notice?: string) => void }) {
  const [pin, setPin] = useState('');
  const [code, setCode] = useState('');
  const [recovering, setRecovering] = useState(false);
  const [hasRecovery, setHasRecovery] = useState(true);
  const [busy, setBusy] = useState(false);
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    api
      .lockStatus()
      .then((s) => setHasRecovery(s.hasRecovery))
      .catch(() => {});
  }, []);

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

  const recover = async () => {
    setBusy(true);
    setError(null);
    try {
      await api.recover(code);
      onUnlocked('The PIN was turned off with your recovery code. Set a new one in Settings, under PIN.');
    } catch (e) {
      setError((e as Error).message);
      setCode('');
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
          if (recovering) {
            if (code) void recover();
          } else if (pin) void submit();
        }}
      >
        <h1>
          <img src="/favicon.svg" alt="" width={32} height={32} /> Budget4
        </h1>
        {recovering ? (
          <>
            <label>
              Recovery code
              <input
                className="mono"
                autoFocus
                value={code}
                onChange={(e) => setCode(e.target.value)}
                disabled={busy}
                autoComplete="off"
                spellCheck={false}
                placeholder="XXXX-XXXX-XXXX-XXXX-XXXX-XXXX"
              />
            </label>
            <p className="muted small">
              This is the code shown when the PIN was set. Using it turns the PIN off, and you can set a new one afterwards.
            </p>
            <div className="row end">
              <button type="button" className="link" onClick={() => { setRecovering(false); setError(null); setCode(''); }} disabled={busy}>
                Back to PIN
              </button>
              <button className="primary" disabled={busy || !code}>
                {busy ? 'Checking…' : 'Reset PIN'}
              </button>
            </div>
          </>
        ) : (
          <>
            <label>
              Enter your PIN
              <input type="password" autoFocus value={pin} onChange={(e) => setPin(e.target.value)} disabled={busy} autoComplete="off" />
            </label>
            <div className="row end">
              {hasRecovery ? (
                <button type="button" className="link" onClick={() => { setRecovering(true); setError(null); }} disabled={busy}>
                  Forgot your PIN?
                </button>
              ) : (
                <span className="muted small">No recovery code was made for this PIN. See Settings, under PIN.</span>
              )}
              <button className="primary" disabled={busy || !pin}>
                {busy ? 'Checking…' : 'Unlock'}
              </button>
            </div>
          </>
        )}
        {error && <p className="error">{error}</p>}
      </form>
    </div>
  );
}

/** Settings section: turn the PIN on, change it, or remove it. */
export function PinSettings({ setToast }: { setToast: (t: string) => void }) {
  const [enabled, setEnabled] = useState<boolean | null>(null);
  const [hasRecovery, setHasRecovery] = useState(false);
  const [mode, setMode] = useState<'set' | 'remove' | null>(null);
  const [current, setCurrent] = useState('');
  const [pin, setPin] = useState('');
  const [confirm, setConfirm] = useState('');
  const [newCode, setNewCode] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const load = () =>
    api
      .lockStatus()
      .then((s) => {
        setEnabled(s.enabled);
        setHasRecovery(s.hasRecovery);
      })
      .catch(() => {});

  useEffect(() => {
    void load();
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
        const saved = await api.setPin(pin, enabled ? current : undefined);
        setToast(enabled ? 'PIN changed' : 'PIN turned on. Budget4 will ask for it when it starts.');
        setEnabled(true);
        setHasRecovery(true);
        setNewCode(saved.recoveryCode);
      } else {
        await api.removePin(current);
        setToast('PIN removed');
        setEnabled(false);
        setHasRecovery(false);
      }
      reset();
    } catch (e) {
      setError((e as Error).message);
    }
  };

  return (
    <section>
      <h3>PIN</h3>
      {newCode && (
        <div className="recovery">
          <p>
            <b>Your recovery code.</b> If you forget the PIN, this code turns it off so you can set a new one. It's shown only now, so write it down
            and keep it somewhere safe.
          </p>
          <code className="recovery-code">{newCode}</code>
          <div className="row end">
            <button className="primary" onClick={() => setNewCode(null)}>
              I've written it down
            </button>
          </div>
        </div>
      )}
      <p>
        {enabled
          ? 'Budget4 asks for a PIN every time it starts.'
          : 'Require a PIN (or password) every time Budget4 starts.'}{' '}
        <span className="muted small">
          This locks the app only: the budget files stay in YNAB 4's format, readable by anyone with access to the folder.
          {enabled && !hasRecovery && ' This PIN has no recovery code, so changing it will make one.'}
          {' '}Without a recovery code, a forgotten PIN is reset by deleting the "pinHash" line from .budget4\config.json in your user folder.
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
