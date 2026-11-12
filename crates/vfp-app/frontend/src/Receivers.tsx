import { useEffect, useState } from "react";
import { api, errText, Receiver } from "./api";

export default function Receivers() {
  const [list, setList] = useState<Receiver[]>([]);
  const [text, setText] = useState("");
  const [msg, setMsg] = useState("");
  const load = () => api.receivers().then(setList).catch((x) => setMsg(errText(x)));
  useEffect(() => {
    load();
  }, []);

  const add = async () => {
    try {
      const n = await api.addReceivers(text);
      setText("");
      setMsg(`Added ${n} receiver${n === 1 ? "" : "s"}.`);
      load();
    } catch (x) {
      setMsg(errText(x));
    }
  };

  return (
    <div className="page">
      <div className="card">
        <h2>Add receivers</h2>
        <p className="muted">
          One person per line. You can add a note after a comma, for example <code>Anna, press office</code>.
        </p>
        <textarea value={text} onChange={(e) => setText(e.target.value)} />
        <div className="row" style={{ marginTop: 10 }}>
          <button className="primary" disabled={!text.trim()} onClick={add}>
            Add
          </button>
          <span className={msg.startsWith("Added") ? "ok" : "err"}>{msg}</span>
        </div>
      </div>
      <div className="card">
        <h2>
          {list.length} receiver{list.length === 1 ? "" : "s"}
        </h2>
        {list.map((r) => (
          <div key={r.id} className="row">
            <strong>{r.name}</strong>
            <span className="muted">{r.note}</span>
          </div>
        ))}
      </div>
    </div>
  );
}
