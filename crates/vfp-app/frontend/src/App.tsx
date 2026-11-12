import { useCallback, useEffect, useState } from "react";
import { api, AppState } from "./api";
import Setup from "./Setup";
import MakeCopies from "./MakeCopies";
import Investigate from "./Investigate";
import Receivers from "./Receivers";
import Workspace from "./Workspace";

const TABS = [
  ["copies", "Make copies"],
  ["investigate", "Investigate a leak"],
  ["receivers", "Receivers"],
  ["workspace", "Workspace"],
] as const;
type Tab = (typeof TABS)[number][0];

export default function App() {
  const [state, setState] = useState<AppState | null>(null);
  const [tab, setTab] = useState<Tab>("copies");
  const refresh = useCallback(() => api.state().then(setState), []);
  useEffect(() => {
    refresh();
  }, [refresh]);

  if (!state) return null;
  if (state.mode === "none") return <Setup onDone={refresh} />;

  return (
    <div className="app">
      <header>
        <h1>VisualFingerprint</h1>
        <nav>
          {TABS.map(([id, label]) => (
            <button key={id} className={tab === id ? "on" : ""} onClick={() => setTab(id)}>
              {label}
            </button>
          ))}
        </nav>
        <span className="badge" title="Where your originals and your key are kept">
          {state.place}
        </span>
      </header>
      <main>
        {tab === "copies" && <MakeCopies />}
        {tab === "investigate" && <Investigate />}
        {tab === "receivers" && <Receivers />}
        {tab === "workspace" && <Workspace state={state} onChange={refresh} />}
      </main>
    </div>
  );
}
