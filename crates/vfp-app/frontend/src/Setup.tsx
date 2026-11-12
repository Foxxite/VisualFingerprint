import { useState } from "react";
import { api, errText, pickSave } from "./api";

export default function Setup({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<"choose" | "server" | "backup">("choose");
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const local = async () => {
    try {
      await api.setupLocal();
      setStep("backup");
    } catch (x) {
      setMsg(errText(x));
    }
  };
  const connect = async () => {
    setBusy(true);
    setMsg("");
    try {
      await api.connect(url, token);
      onDone();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };
  const backup = async () => {
    const dest = await pickSave("Save a backup of your key", "vfp-master.key");
    if (!dest) return;
    try {
      await api.exportKey(dest);
      setMsg("Backup saved. Keep it somewhere safe and private.");
    } catch (x) {
      setMsg(errText(x));
    }
  };

  return (
    <main>
      <div className="page" style={{ marginTop: 40 }}>
        <h1 style={{ margin: 0 }}>Welcome to VisualFingerprint</h1>
        <p className="muted">
          Give each person a copy that looks the same but carries a hidden mark. If a copy leaks, find out who leaked it.
        </p>

        {step === "choose" && (
          <div className="grid2">
            <button className="card choice" onClick={local}>
              <h2>This device only</h2>
              <span className="muted">Your files and your secret key stay on this computer. Works without internet.</span>
            </button>
            <button className="card choice" onClick={() => setStep("server")}>
              <h2>Connect to my server</h2>
              <span className="muted">Use one workspace on all your devices. Originals stay on the server.</span>
            </button>
          </div>
        )}

        {step === "server" && (
          <div className="card">
            <h2>Connect to your server</h2>
            <label>Server address</label>
            <input type="text" placeholder="https://vfp.example.com" value={url} onChange={(e) => setUrl(e.target.value)} />
            <label>Token</label>
            <input type="password" value={token} onChange={(e) => setToken(e.target.value)} />
            <div className="row" style={{ marginTop: 14 }}>
              <button className="primary" disabled={busy || !url || !token} onClick={connect}>
                Connect
              </button>
              <button className="secondary" onClick={() => setStep("choose")}>
                Back
              </button>
            </div>
          </div>
        )}

        {step === "backup" && (
          <div className="card">
            <h2>Back up your secret key</h2>
            <p>
              Your key is the only way to find out who leaked a file. If you lose it, you cannot trace a leak. Do not share it.
            </p>
            <div className="row">
              <button className="primary" onClick={backup}>
                Save a backup…
              </button>
              <button className="secondary" onClick={onDone}>
                Continue
              </button>
            </div>
          </div>
        )}
        {msg && <p className={msg.startsWith("Backup") ? "ok" : "err"}>{msg}</p>}
      </div>
    </main>
  );
}
