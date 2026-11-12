import { useState } from "react";
import { api, errText } from "./api";
import Passphrase from "./Passphrase";

/// A new server has no vault yet. Make one with a passphrase that stays on this device.
export default function NewServerVault({ onDone }: { onDone: () => void }) {
  const [pass, setPass] = useState<string | null>(null);
  const [busy, setBusy] = useState(false);
  const [zk, setZk] = useState(false);
  const [msg, setMsg] = useState("");
  const go = async () => {
    if (!pass) return;
    setBusy(true);
    setMsg("");
    try {
      await api.createServerVault(pass, zk);
      onDone();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };
  return (
    <main>
      <div className="page" style={{ marginTop: 40, maxWidth: 520 }}>
        <h1 style={{ margin: 0 }}>Protect your server</h1>
        <p className="muted">
          This server has no encryption set up. Choose a passphrase. Your files on the server are then encrypted. You need the
          passphrase on every device. Only an admin token can do this step.
        </p>
        <div className="card">
          <label style={{ display: "block", marginBottom: 8 }}>
            <input type="radio" checked={!zk} onChange={() => setZk(false)} /> <b>Protected</b> (default). The server gets the key
            for each request, over HTTPS, and does the work. Fast on phones.
          </label>
          <label style={{ display: "block", marginBottom: 14 }}>
            <input type="radio" checked={zk} onChange={() => setZk(true)} /> <b>Zero-knowledge</b>. The key never leaves your
            devices. The server only stores encrypted data. Each device downloads what it needs.
          </label>
          <Passphrase onValid={setPass} />
          <div className="row" style={{ marginTop: 14 }}>
            <button className="primary" disabled={!pass || busy} onClick={go}>
              {busy ? "Working…" : "Encrypt the server"}
            </button>
            <button
              className="secondary"
              onClick={async () => {
                await api.disconnect();
                onDone();
              }}
            >
              Disconnect
            </button>
          </div>
        </div>
        {msg && <p className="err">{msg}</p>}
      </div>
    </main>
  );
}
