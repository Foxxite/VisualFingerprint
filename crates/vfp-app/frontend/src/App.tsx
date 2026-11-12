import { useCallback, useEffect, useState } from "react";
import { api, AppState } from "./api";
import Setup from "./Setup";
import MakeCopies from "./MakeCopies";
import Investigate from "./Investigate";
import Receivers from "./Receivers";
import Workspace from "./Workspace";
import Unlock from "./Unlock";
import NewServerVault from "./NewServerVault";

const IDLE_MS = 15 * 60 * 1000;

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

  const locked = !!state?.locked;
  useEffect(() => {
    if (!state?.encrypted || state.locked) return;
    let t = window.setTimeout(lockNow, IDLE_MS);
    const bump = () => {
      window.clearTimeout(t);
      t = window.setTimeout(lockNow, IDLE_MS);
    };
    const evs = ["mousemove", "keydown", "click"] as const;
    evs.forEach((n) => window.addEventListener(n, bump));
    return () => {
      window.clearTimeout(t);
      evs.forEach((n) => window.removeEventListener(n, bump));
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [state?.encrypted, state?.locked]);

  async function lockNow() {
    await api.lock();
    refresh();
  }

  if (!state) return null;
  if (state.mode === "none") return <Setup onDone={refresh} />;
  if (state.needs_vault) return <NewServerVault onDone={refresh} />;
  if (locked) return <Unlock state={state} onDone={refresh} />;

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
          {state.encrypted ? "🔒 " : ""}
          {state.place}
        </span>
        {state.encrypted && (
          <button className="secondary" onClick={lockNow} title="Forget the passphrase until you enter it again">
            Lock
          </button>
        )}
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
