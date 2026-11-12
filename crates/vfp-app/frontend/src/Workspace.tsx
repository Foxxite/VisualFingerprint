import { useState } from "react";
import { api, AppState, errText, pickSave } from "./api";
import Passphrase from "./Passphrase";
import Workspaces from "./Workspaces";

export default function Workspace({ state, onChange }: { state: AppState; onChange: () => void }) {
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [msg, setMsg] = useState("");
  const [old, setOld] = useState("");
  const [fresh, setFresh] = useState<string | null>(null);
  const [formKey, setFormKey] = useState(0);
  const [busy, setBusy] = useState(false);

  const change = async () => {
    if (!fresh) return;
    setBusy(true);
    try {
      await api.changePassphrase(old, fresh);
      setMsg("Passphrase changed. Save a new backup of your key file.");
      setOld("");
      setFormKey(formKey + 1);
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };
  const encrypt = async () => {
    if (!fresh) return;
    setBusy(true);
    try {
      await api.encryptWorkspace(fresh);
      setMsg("Encrypted. The plain originals were removed. Save a backup of your key file.");
      onChange();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };

  const backup = async () => {
    const dest = await pickSave("Save a backup of your key file", "vfp-key-backup");
    if (!dest) return;
    try {
      await api.exportKey(dest);
      setMsg("Backup saved.");
    } catch (x) {
      setMsg(errText(x));
    }
  };
  const connect = async () => {
    try {
      setMsg(await api.connect(url, token));
      onChange();
    } catch (x) {
      setMsg(errText(x));
    }
  };
  const leave = async () => {
    await api.disconnect();
    setMsg("");
    onChange();
  };

  return (
    <div className="page">
      <Workspaces onChange={onChange} />
      <div className="card">
        <h2>Where your data is</h2>
        <p>
          {state.mode === "server" ? (
            <>
              Your originals and your key are on the server <strong>{state.server_url}</strong>. This device holds no original and no key.
            </>
          ) : (
            <>Your originals and your key are on this device.</>
          )}
        </p>
        <p className="muted">
          App folder: <code>{state.home}</code>
        </p>
      </div>

      <div className="card">
        <h2>Protection</h2>
        {state.encrypted ? (
          <p>
            🔒 Encrypted. {state.mode === "server" ? "The server holds only encrypted files." : "Originals and key are encrypted on this device."}{" "}
            The app forgets the passphrase after 15 minutes without use, or when you press Lock.
          </p>
        ) : (
          <p className="warn">Not encrypted. Anyone who can read the folder can read your originals.</p>
        )}
        {state.mode === "local" && state.encrypted && (
          <>
            <h3>Change passphrase</h3>
            <label>Current passphrase</label>
            <input type="password" value={old} onChange={(e) => setOld(e.target.value)} />
            <Passphrase key={formKey} onValid={setFresh} />
            <button className="primary" style={{ marginTop: 10 }} disabled={!old || !fresh || busy} onClick={change}>
              {busy ? "Working…" : "Change passphrase"}
            </button>
          </>
        )}
        {state.mode === "local" && !state.encrypted && (
          <>
            <h3>Encrypt this workspace</h3>
            <Passphrase key={formKey} onValid={setFresh} />
            <button className="primary" style={{ marginTop: 10 }} disabled={!fresh || busy} onClick={encrypt}>
              {busy ? "Working…" : "Encrypt now"}
            </button>
          </>
        )}
      </div>

      {state.mode === "local" && (
        <div className="card">
          <h2>Back up your key</h2>
          <p className="muted">Without your key you cannot trace a leak.</p>
          <button className="primary" onClick={backup}>
            Save a backup…
          </button>
        </div>
      )}

      <div className="card">
        <h2>{state.mode === "server" ? "Change server" : "Use a server instead"}</h2>
        <label>Server address</label>
        <input type="text" placeholder="https://vfp.example.com" value={url} onChange={(e) => setUrl(e.target.value)} />
        <label>Token</label>
        <input type="password" value={token} onChange={(e) => setToken(e.target.value)} />
        <div className="row" style={{ marginTop: 12 }}>
          <button className="primary" disabled={!url || !token} onClick={connect}>
            Connect
          </button>
          {state.mode === "server" && (
            <button className="secondary" onClick={leave}>
              Go back to this device
            </button>
          )}
        </div>
        {state.mode === "local" && <p className="muted">Your local workspace is kept. You can come back to it any time.</p>}
      </div>
      {msg && <p className={msg.includes("saved") || msg.startsWith("Passphrase changed") || msg.startsWith("Encrypted") || msg.startsWith("Connected") ? "ok" : "err"}>{msg}</p>}
    </div>
  );
}
