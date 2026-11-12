import { useState } from "react";
import { api, AppState, errText, pickSave } from "./api";

export default function Workspace({ state, onChange }: { state: AppState; onChange: () => void }) {
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [msg, setMsg] = useState("");

  const backup = async () => {
    const dest = await pickSave("Save a backup of your key", "vfp-master.key");
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
      {msg && <p className={msg.includes("saved") || msg.startsWith("Connected") ? "ok" : "err"}>{msg}</p>}
    </div>
  );
}
