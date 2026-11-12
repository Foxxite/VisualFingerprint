import { useEffect, useState } from "react";
import { api, errText, isAndroid, pickFolder, WsItem } from "./api";

/// The list of workspaces: open, add, rename, forget. Every workspace has its own key, server link and data.
export default function Workspaces({ onChange, hideIfEmpty }: { onChange: () => void; hideIfEmpty?: boolean }) {
  const [items, setItems] = useState<WsItem[]>([]);
  const [name, setName] = useState("");
  const [folder, setFolder] = useState("");
  const [edit, setEdit] = useState<string | null>(null);
  const [editName, setEditName] = useState("");
  const [msg, setMsg] = useState("");

  const load = () => api.listWorkspaces().then(setItems).catch((x) => setMsg(errText(x)));
  useEffect(() => {
    load();
  }, []);

  const run = async (f: () => Promise<unknown>, reload = false) => {
    try {
      await f();
      setMsg("");
      if (reload) onChange();
      else load();
    } catch (x) {
      setMsg(errText(x));
    }
  };
  const browse = async () => {
    const f = await pickFolder("Choose a folder for the new workspace");
    if (f) setFolder(f);
  };

  if (hideIfEmpty && items.length === 0) return null;
  return (
    <div className="card">
      <h2>Workspaces</h2>
      <p className="muted">Each workspace has its own receivers, originals, key and server. Switching locks the current one.</p>
      <ul className="ws-list">
        {items.map((w) => (
          <li key={w.path} className={w.current ? "on" : ""}>
            <div style={{ flex: 1, minWidth: 0 }}>
              {edit === w.path ? (
                <input type="text" value={editName} autoFocus onChange={(e) => setEditName(e.target.value)} />
              ) : (
                <strong>{w.name}</strong>
              )}
              <div className="muted">
                {w.server ? `Server ${w.server}` : w.ready ? "This device" : "Not set up yet"} · <code>{w.path}</code>
              </div>
            </div>
            {edit === w.path ? (
              <>
                <button className="primary" onClick={() => run(() => api.renameWorkspace(w.path, editName).then(() => setEdit(null)))}>
                  Save
                </button>
                <button className="secondary" onClick={() => setEdit(null)}>
                  Cancel
                </button>
              </>
            ) : (
              <>
                {w.current ? (
                  <span className="badge">Open now</span>
                ) : (
                  <button className="primary" onClick={() => run(() => api.switchWorkspace(w.path), true)}>
                    Open
                  </button>
                )}
                <button
                  className="secondary"
                  onClick={() => {
                    setEdit(w.path);
                    setEditName(w.name);
                  }}
                >
                  Rename
                </button>
                {!w.current && (
                  <button className="secondary" title="Remove from the list. The folder stays on disk." onClick={() => run(() => api.removeWorkspace(w.path))}>
                    Forget
                  </button>
                )}
              </>
            )}
          </li>
        ))}
      </ul>
      <h3>Add a workspace</h3>
      <label>Name</label>
      <input type="text" placeholder="Client A" value={name} onChange={(e) => setName(e.target.value)} />
      <label>Folder</label>
      <div className="row">
        <input type="text" style={{ flex: 1 }} value={folder} onChange={(e) => setFolder(e.target.value)} />
        {!isAndroid && (
          <button className="secondary" onClick={browse}>
            Browse…
          </button>
        )}
      </div>
      <button className="primary" style={{ marginTop: 10 }} disabled={!name.trim() || (!folder.trim() && !isAndroid)} onClick={() => run(async () => {
        // On Android there is no folder picker: use a new folder next to the default workspace.
        let f = folder.trim();
        if (!f) f = `${await api.defaultWorkspace()}-${name.trim().toLowerCase().replace(/[^a-z0-9]+/g, "-")}`;
        await api.addWorkspace(name, f);
      }, true)}>
        Add and set up
      </button>
      {msg && <p className="err">{msg}</p>}
    </div>
  );
}
