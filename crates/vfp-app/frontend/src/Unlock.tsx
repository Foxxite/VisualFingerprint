import { useState } from "react";
import { api, AppState, errText } from "./api";

/// Shown when the workspace or the server vault is locked. Also creates a vault on a new server.
export default function Unlock({ state, onDone }: { state: AppState; onDone: () => void }) {
  const [pass, setPass] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const go = async () => {
    setBusy(true);
    setMsg("");
    try {
      await api.unlock(pass);
      setPass("");
      onDone();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };

  return (
    <main>
      <div className="page" style={{ marginTop: 60, maxWidth: 460 }}>
        <h1 style={{ margin: 0 }}>Unlock</h1>
        <p className="muted">
          {state.mode === "server" ? `Your files on ${state.server_url} are encrypted.` : "Your workspace is encrypted."} Enter your passphrase.
          The passphrase never leaves this device.
        </p>
        <div className="card">
          <label>Passphrase</label>
          <input
            type="password"
            autoFocus
            value={pass}
            onChange={(e) => setPass(e.target.value)}
            onKeyDown={(e) => e.key === "Enter" && pass && !busy && go()}
          />
          <div className="row" style={{ marginTop: 14 }}>
            <button className="primary" disabled={!pass || busy} onClick={go}>
              {busy ? "Unlocking…" : "Unlock"}
            </button>
          </div>
        </div>
        {msg && <p className="err">{msg}</p>}
      </div>
    </main>
  );
}
