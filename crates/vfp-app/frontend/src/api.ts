import { invoke } from "@tauri-apps/api/core";
import { listen } from "@tauri-apps/api/event";
import { open, save } from "@tauri-apps/plugin-dialog";

export type Mode = "none" | "local" | "server";
export interface AppState {
  mode: Mode; place: string; server_url: string | null; home: string;
  encrypted: boolean; locked: boolean; needs_vault: boolean; zero_knowledge: boolean;
}
export interface Strength { bits: number; ok: boolean; advice: string[] }
export interface Receiver { id: number; name: string; note: string }
export interface Asset { id: number; name: string; kind: string; m: number; max_receivers: number; copies: number }
export interface Plan { kind: string; m: number; guaranteed_colluders: number; notes: string[] }
export interface TraceResult {
  asset: string; kind: string; ranking: [string, number][]; threshold: number;
  flagged: string[]; verdict: string; warnings: string[]; evidence: unknown;
}

export const api = {
  state: () => invoke<AppState>("app_state"),
  setupLocal: (folder: string | null, passphrase: string | null) => invoke<string>("setup_local", { folder, passphrase }),
  strength: (passphrase: string) => invoke<Strength>("check_strength", { passphrase }),
  generatePassphrase: () => invoke<string>("generate_passphrase"),
  unlock: (passphrase: string) => invoke<void>("unlock", { passphrase }),
  lock: () => invoke<void>("lock"),
  createServerVault: (passphrase: string, zeroKnowledge: boolean) => invoke<void>("create_server_vault", { passphrase, zeroKnowledge }),
  changePassphrase: (old: string, nw: string) => invoke<void>("change_passphrase", { old, new: nw }),
  encryptWorkspace: (passphrase: string) => invoke<string>("encrypt_workspace", { passphrase }),
  defaultWorkspace: () => invoke<string>("default_workspace"),
  connect: (url: string, token: string) => invoke<string>("connect_server", { url, token }),
  disconnect: () => invoke<void>("disconnect_server"),
  exportKey: (dest: string) => invoke<void>("export_key", { dest }),
  receivers: () => invoke<Receiver[]>("receivers"),
  addReceivers: (text: string) => invoke<number>("add_receivers", { text }),
  assets: () => invoke<Asset[]>("assets"),
  preview: (path: string, maxReceivers: number) => invoke<Plan>("preview_file", { path, maxReceivers }),
  addAsset: (path: string, name: string | null, maxReceivers: number) =>
    invoke<[Asset, Plan]>("add_asset", { path, name, maxReceivers }),
  makeCopies: (asset: string, receivers: string[], outdir: string) =>
    invoke<[string, string][]>("make_copies", { asset, receivers, outdir }),
  trace: (path: string, asset: string | null) => invoke<TraceResult>("trace_file", { path, asset }),
  saveReport: (result: TraceResult, leaked: string, dest: string) =>
    invoke<void>("save_report", { result, leaked, dest }),
};

export const onProgress = (f: (done: number, total: number) => void) =>
  listen<{ done: number; total: number }>("progress", (ev) => f(ev.payload.done, ev.payload.total));

export const pickFile = async (title: string): Promise<string | null> => {
  const r = await open({ title, multiple: false, directory: false });
  return typeof r === "string" ? r : null;
};
export const pickFolder = async (title: string): Promise<string | null> => {
  const r = await open({ title, multiple: false, directory: true });
  return typeof r === "string" ? r : null;
};
export const pickSave = (title: string, defaultPath: string) => save({ title, defaultPath });

export const errText = (x: unknown) => (typeof x === "string" ? x : x instanceof Error ? x.message : JSON.stringify(x));
