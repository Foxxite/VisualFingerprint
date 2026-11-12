import { useEffect, useState } from "react";
import { api, errText, Strength } from "./api";

/// A new-passphrase box with a strength meter and a "Make one for me" button.
/// `onValid` gets the passphrase when it is strong enough and both boxes match, else null.
export default function Passphrase({ onValid }: { onValid: (p: string | null) => void }) {
  const [a, setA] = useState("");
  const [b, setB] = useState("");
  const [show, setShow] = useState(false);
  const [s, setS] = useState<Strength | null>(null);
  const [err, setErr] = useState("");

  useEffect(() => {
    let live = true;
    if (!a) {
      setS(null);
      return;
    }
    api.strength(a).then((r) => live && setS(r)).catch((x) => live && setErr(errText(x)));
    return () => {
      live = false;
    };
  }, [a]);

  const match = a === b;
  useEffect(() => {
    onValid(s?.ok && match ? a : null);
  }, [s, a, match, onValid]);

  const make = async () => {
    const p = await api.generatePassphrase();
    setA(p);
    setB(p);
    setShow(true);
  };

  const pct = s ? Math.min(100, Math.round((s.bits / 64) * 100)) : 0;
  return (
    <div>
      <label>Passphrase</label>
      <input type={show ? "text" : "password"} value={a} onChange={(e) => setA(e.target.value)} autoComplete="new-password" />
      <div className="bar" style={{ marginTop: 6 }} aria-label="Passphrase strength">
        <div style={{ width: `${pct}%`, background: s?.ok ? "var(--ok)" : "var(--warn)" }} />
      </div>
      <p className="muted" style={{ margin: "4px 0" }}>
        {s ? `${Math.round(s.bits)}${s.capped ? "+" : ""} bits. ${s.ok ? "Strong enough." : "Need 64 bits."}` : "Use 8 random words, or press the button."}
      </p>
      {s && !s.ok && s.advice.map((t) => <div key={t} className="warn">{t}</div>)}
      <label>Type it again</label>
      <input type={show ? "text" : "password"} value={b} onChange={(e) => setB(e.target.value)} autoComplete="new-password" />
      {b && !match && <div className="err">The two passphrases differ.</div>}
      <div className="row" style={{ marginTop: 10 }}>
        <button className="secondary" onClick={make}>
          Make one for me
        </button>
        <button className="secondary" onClick={() => setShow(!show)}>
          {show ? "Hide" : "Show"}
        </button>
      </div>
      <p className="warn">
        Write the passphrase down. If you lose it, your files <strong>cannot</strong> be opened. Nobody can recover it.
      </p>
      {err && <p className="err">{err}</p>}
    </div>
  );
}
