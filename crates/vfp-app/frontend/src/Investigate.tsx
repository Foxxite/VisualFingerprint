import { useEffect, useRef, useState } from "react";
import { getCurrentWebview } from "@tauri-apps/api/webview";
import { api, Asset, errText, pickFile, pickSave, TraceResult } from "./api";

export default function Investigate() {
  const [file, setFile] = useState<string | null>(null);
  const [assets, setAssets] = useState<Asset[]>([]);
  const [asset, setAsset] = useState("");
  const [res, setRes] = useState<TraceResult | null>(null);
  const [busy, setBusy] = useState(false);
  const [over, setOver] = useState(false);
  const [msg, setMsg] = useState("");
  const busyRef = useRef(false);
  busyRef.current = busy;

  useEffect(() => {
    api.assets().then(setAssets).catch((x) => setMsg(errText(x)));
  }, []);
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent((ev) => {
      const p = ev.payload;
      if (p.type === "over" || p.type === "enter") setOver(true);
      else if (p.type === "leave") setOver(false);
      else if (p.type === "drop") {
        setOver(false);
        const path = p.paths[0];
        if (path && !busyRef.current) {
          setFile(path);
          setRes(null);
        }
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);

  const choose = async () => {
    const p = await pickFile("Choose the leaked file");
    if (p) {
      setFile(p);
      setRes(null);
    }
  };
  const run = async () => {
    if (!file) return;
    setBusy(true);
    setMsg("");
    setRes(null);
    try {
      setRes(await api.trace(file, asset || null));
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };
  const report = async () => {
    if (!res || !file) return;
    const dest = await pickSave("Save the report", "trace-report.txt");
    if (!dest) return;
    try {
      await api.saveReport(res, file, dest);
      setMsg("Report saved.");
    } catch (x) {
      setMsg(errText(x));
    }
  };

  const top = res ? Math.max(res.threshold * 1.6, ...res.ranking.map((r) => r[1])) : 1;
  return (
    <div className="page">
      <div className={"drop" + (over ? " over" : "")}>
        {file ? (
          <>
            <strong>{file.split(/[\\/]/).pop()}</strong>
            <br />
            <code>{file}</code>
          </>
        ) : (
          "Drop the leaked file here"
        )}
        <div className="row" style={{ justifyContent: "center", marginTop: 14 }}>
          <button className="secondary" disabled={busy} onClick={choose}>
            Choose a file…
          </button>
          <select value={asset} onChange={(e) => setAsset(e.target.value)}>
            <option value="">Find the matching original myself</option>
            {assets.map((a) => (
              <option key={a.id} value={a.name}>
                {a.name}
              </option>
            ))}
          </select>
          <button className="primary" disabled={!file || busy} onClick={run}>
            {busy ? "Checking… (a video can take a minute)" : "Find the leaker"}
          </button>
        </div>
      </div>
      {msg && <p className={msg === "Report saved." ? "ok" : "err"}>{msg}</p>}

      {res && (
        <div className="card">
          <div className="verdict">{res.verdict}</div>
          <p className="muted">
            Original: {res.asset} ({res.kind})
          </p>
          {res.warnings.map((w, i) => (
            <p key={i} className="warn">
              {w}
            </p>
          ))}
          {res.ranking.slice(0, 8).map(([name, s]) => (
            <div className="score" key={name}>
              <span>{name}</span>
              <div className="track">
                <div className={"fill" + (s > res.threshold ? " hit" : "")} style={{ width: `${Math.max(0, (100 * s) / top)}%` }} />
                <div className="line" style={{ left: `${(100 * res.threshold) / top}%` }} title="Accusation threshold" />
              </div>
              <span>{s.toFixed(1)}</span>
            </div>
          ))}
          <p className="muted">
            The black line is the threshold. A receiver must pass it to be named. A score is statistical evidence, not proof.
          </p>
          <button className="secondary" onClick={report}>
            Save report…
          </button>
        </div>
      )}
    </div>
  );
}
