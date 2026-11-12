import { useState } from "react";
import { api, errText, pickFolder, pickSave } from "./api";
import Passphrase from "./Passphrase";
import Workspaces from "./Workspaces";

export default function Setup({ onDone }: { onDone: () => void }) {
  const [step, setStep] = useState<"choose" | "folder" | "protect" | "server" | "backup">("choose");
  const [url, setUrl] = useState("");
  const [token, setToken] = useState("");
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const [folder, setFolder] = useState("");
  const [dflt, setDflt] = useState("");

  const toFolder = async () => {
    setDflt(await api.defaultWorkspace());
    setStep("folder");
  };
  const browse = async () => {
    const f = await pickFolder("Where should your originals and key be stored?");
    if (f) setFolder(f);
  };
  const [pass, setPass] = useState<string | null>(null);
  const local = async (passphrase: string | null) => {
    setBusy(true);
    setMsg("");
    try {
      await api.setupLocal(folder || null, passphrase);
      setStep("backup");
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
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
    const dest = await pickSave("Save a backup of your key file", "vfp-key-backup");
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

        {step === "choose" && <Workspaces hideIfEmpty onChange={onDone} />}
        {step === "choose" && (
          <div className="grid2">
            <button className="card choice" onClick={toFolder}>
              <h2>This device only</h2>
              <span className="muted">Your files and your secret key stay on this computer. Works without internet.</span>
            </button>
            <button className="card choice" onClick={() => setStep("server")}>
              <h2>Connect to my server</h2>
              <span className="muted">Use one workspace on all your devices. Originals stay on the server.</span>
            </button>
          </div>
        )}

        {step === "folder" && (
          <div className="card">
            <h2>Where should your originals be stored?</h2>
            <p>
              The app keeps one copy of each original here, and your secret key. Pick a folder with enough space. A folder that
              is backed up is best.
            </p>
            <code>{folder || dflt}</code>
            <div className="row" style={{ marginTop: 14 }}>
              <button className="secondary" onClick={browse}>
                Choose a folder…
              </button>
              <button className="primary" onClick={() => setStep("protect")}>
                Use this folder
              </button>
              <button className="secondary" onClick={() => setStep("choose")}>
                Back
              </button>
            </div>
          </div>
        )}

        {step === "protect" && (
          <div className="card">
            <h2>Protect your files with a passphrase</h2>
            <p>
              Your originals, your secret key and your receiver names are encrypted on this device. You need the passphrase to open
              them.
            </p>
            <Passphrase onValid={setPass} />
            <div className="row" style={{ marginTop: 14 }}>
              <button className="primary" disabled={!pass || busy} onClick={() => local(pass)}>
                {busy ? "Working…" : "Encrypt and continue"}
              </button>
              <button className="secondary" onClick={() => setStep("folder")}>
                Back
              </button>
            </div>
            <p className="muted">
              <a
                href="#"
                onClick={(e) => {
                  e.preventDefault();
                  if (window.confirm("Without a passphrase, anyone who can read this folder can read your originals. Continue without encryption?")) local(null);
                }}
              >
                Continue without encryption
              </a>{" "}
              (not for sensitive files)
            </p>
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
            <h2>Back up your key file</h2>
            <p>
              This file is the only way to find out who leaked a file. If you lose it, you cannot trace a leak. Do not share it.
              If your workspace is encrypted, it also needs your passphrase.
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
