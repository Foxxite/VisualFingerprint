import { useEffect, useState } from "react";
import { api, Asset, baseName, errText, isAndroid, onProgress, pickFile, pickFolder, pickSave, Plan, Receiver } from "./api";

export default function MakeCopies() {
  const [assets, setAssets] = useState<Asset[]>([]);
  const [receivers, setReceivers] = useState<Receiver[]>([]);
  const [asset, setAsset] = useState("");
  const [picked, setPicked] = useState<Set<string>>(new Set());
  const [newFile, setNewFile] = useState<{ path: string; name: string; plan: Plan } | null>(null);
  const [maxR, setMaxR] = useState(1000);
  const [prog, setProg] = useState<[number, number] | null>(null);
  const [done, setDone] = useState<[string, string][]>([]);
  const [msg, setMsg] = useState("");
  const [busy, setBusy] = useState(false);

  const load = () =>
    Promise.all([api.assets(), api.receivers()])
      .then(([a, r]) => {
        setAssets(a);
        setReceivers(r);
      })
      .catch((x) => setMsg(errText(x)));
  useEffect(() => {
    load();
  }, []);
  useEffect(() => {
    const p = onProgress((d, t) => setProg([d, t]));
    return () => {
      p.then((u) => u());
    };
  }, []);

  const preview = async (path: string, n: number) => {
    setMsg("");
    try {
      setNewFile({ path, name: await api.displayName(path), plan: await api.preview(path, n) });
    } catch (x) {
      setMsg(errText(x));
      setNewFile(null);
    }
  };
  const choose = async () => {
    const path = await pickFile("Choose the original file");
    if (path) await preview(path, maxR);
  };
  const register = async () => {
    if (!newFile) return;
    setBusy(true);
    setMsg("Registering. A big video can take a moment…");
    try {
      const [a] = await api.addAsset(newFile.path, null, maxR);
      setNewFile(null);
      setAsset(a.name);
      setMsg(`Registered "${a.name}".`);
      await load();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
  };
  const toggle = (n: string) =>
    setPicked((s) => {
      const c = new Set(s);
      if (c.has(n)) c.delete(n);
      else c.add(n);
      return c;
    });

  const run = async () => {
    // Android has no folder paths: the copies are made in app storage, then saved one by one.
    const dir = isAndroid ? "" : await pickFolder("Where should the copies go?");
    if (dir === null) return;
    setBusy(true);
    setDone([]);
    setProg([0, 1]);
    setMsg("");
    try {
      const made = await api.makeCopies(asset, [...picked], dir);
      setDone(made);
      if (isAndroid) {
        let saved = 0;
        for (const [r, p] of made) {
          setMsg(`Save the copy for ${r} (${saved + 1} of ${made.length})…`);
          const dest = await pickSave(`Save the copy for ${r}`, baseName(p));
          if (!dest) break;
          await api.copyOut(p, dest);
          saved++;
        }
        setMsg(saved === made.length ? "Done. All copies saved." : `Saved ${saved} of ${made.length}. The rest are in the app's storage.`);
      } else {
        setMsg("Done.");
      }
      await load();
    } catch (x) {
      setMsg(errText(x));
    }
    setBusy(false);
    setProg(null);
  };

  const ok = asset && picked.size > 0 && !busy;
  return (
    <div className="page">
      <div className="card">
        <h2>1. Choose the original</h2>
        <div className="row">
          <select value={asset} onChange={(e) => setAsset(e.target.value)} style={{ flex: 1 }}>
            <option value="">— pick a registered file —</option>
            {assets.map((a) => (
              <option key={a.id} value={a.name}>
                {a.name} ({a.kind}, {a.copies} copies made)
              </option>
            ))}
          </select>
          <button className="secondary" disabled={busy} onClick={choose}>
            Add a new file…
          </button>
        </div>
        {newFile && (
          <div style={{ marginTop: 12 }}>
            <strong>{newFile.name}</strong>
            {!newFile.path.startsWith("content://") && (
              <>
                <br />
                <code>{newFile.path}</code>
              </>
            )}
            <p>
              This {newFile.plan.kind} can carry <strong>{newFile.plan.m}</strong> symbols.{" "}
              {newFile.plan.guaranteed_colluders > 0 ? (
                <>
                  Tracing is guaranteed even if up to <strong>{newFile.plan.guaranteed_colluders}</strong> receivers work together.
                </>
              ) : (
                <>Tracing works for one leaker. A group of receivers working together is not guaranteed.</>
              )}
            </p>
            {newFile.plan.notes.map((n, i) => (
              <p key={i} className="warn">
                {n}
              </p>
            ))}
            <div className="row">
              <label style={{ margin: 0 }}>Most receivers you plan for</label>
              <input
                type="number"
                min={2}
                style={{ width: 100 }}
                value={maxR}
                onChange={(e) => setMaxR(Number(e.target.value) || 2)}
                onBlur={() => preview(newFile.path, maxR)}
              />
              <button className="primary" disabled={busy} onClick={register}>
                Register this file
              </button>
            </div>
          </div>
        )}
      </div>

      <div className="card">
        <h2>2. Choose who gets a copy</h2>
        {receivers.length === 0 && <p className="muted">No receivers yet. Add them on the Receivers tab.</p>}
        <div className="row" style={{ marginBottom: 6 }}>
          <button className="secondary" onClick={() => setPicked(new Set(receivers.map((r) => r.name)))}>
            All
          </button>
          <button className="secondary" onClick={() => setPicked(new Set())}>
            None
          </button>
        </div>
        <div className="checks">
          {receivers.map((r) => (
            <label key={r.id}>
              <input type="checkbox" checked={picked.has(r.name)} onChange={() => toggle(r.name)} />
              {r.name}
            </label>
          ))}
        </div>
      </div>

      <div className="card">
        <h2>3. Make the copies</h2>
        <div className="row">
          <button className="primary" disabled={!ok} onClick={run}>
            Make {picked.size || ""} cop{picked.size === 1 ? "y" : "ies"}…
          </button>
          <span className={msg.startsWith("Done.") || msg.startsWith("Registered") ? "ok" : msg ? "err" : "muted"}>{msg}</span>
        </div>
        {prog && (
          <div className="bar" style={{ marginTop: 12 }}>
            <div style={{ width: `${(100 * prog[0]) / Math.max(prog[1], 1)}%` }} />
          </div>
        )}
        {done.map(([r, p]) => (
          <div key={r} style={{ marginTop: 6 }}>
            <strong>{r}</strong> <code>{p}</code>
          </div>
        ))}
      </div>
    </div>
  );
}
