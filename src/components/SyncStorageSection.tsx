import { useCallback, useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { AlertTriangle, CheckCircle2, ChevronRight, Cloud, Database, Loader2, PlugZap } from "lucide-react";

// Where PERSONAL profiles sync on this install: the Submarine Cloud account, or
// the user's own S3-compatible bucket. Per-device, stored by the backend (not in
// localStorage) because Rust does the syncing. The S3 keys never pass through
// here — the backend reads them from an s3cmd config file when it syncs.
// Shared profiles always use the cloud account, whatever is chosen here.

type Backend = "http" | "s3";

interface S3Settings {
  credentials_file: string;
  bucket: string;
  prefix: string;
  endpoint: string | null;
  region: string | null;
  path_style: boolean;
}

interface SyncBackendView {
  backend: Backend;
  s3: S3Settings;
  credentials_ok: boolean;
  credentials_error: string | null;
  endpoint: string | null;
  region: string | null;
}

// "[S3] SIGNATURE_MISMATCH: the storage server…" → "the storage server…"
const clean = (e: unknown) => String(e).replace(/^\[[A-Z0-9_]+\]\s*/, "").replace(/^[A-Z_]+:\s*/, "");

export default function SyncStorageSection({ onChanged }: { onChanged?: () => void }) {
  const [saved, setSaved] = useState<SyncBackendView | null>(null);
  const [backend, setBackend] = useState<Backend>("http");
  const [s3, setS3] = useState<S3Settings | null>(null);
  const [advanced, setAdvanced] = useState(false);
  const [busy, setBusy] = useState<"test" | "save" | null>(null);
  const [err, setErr] = useState<string | null>(null);
  const [ok, setOk] = useState<string | null>(null);

  const adopt = (v: SyncBackendView) => {
    setSaved(v);
    setBackend(v.backend);
    setS3(v.s3);
    setAdvanced(!!(v.s3.endpoint || v.s3.region || !v.s3.path_style));
  };

  const load = useCallback(async () => {
    try { adopt(await invoke<SyncBackendView>("sync_backend_get")); }
    catch (e) { setErr(clean(e)); }
  }, []);
  useEffect(() => { load(); }, [load]);

  if (!s3) {
    return err ? <Message kind="err" text={err} /> : null;
  }

  const set = (patch: Partial<S3Settings>) => { setS3({ ...s3, ...patch }); setOk(null); };
  const dirty = !!saved && (backend !== saved.backend || JSON.stringify(s3) !== JSON.stringify(saved.s3));

  const test = async () => {
    setBusy("test"); setErr(null); setOk(null);
    try { setOk(await invoke<string>("s3_test_connection", { settings: s3 })); }
    catch (e) { setErr(clean(e)); }
    finally { setBusy(null); }
  };

  const save = async () => {
    setBusy("save"); setErr(null); setOk(null);
    try {
      adopt(await invoke<SyncBackendView>("sync_backend_set", { config: { backend, s3 } }));
      setOk(backend === "s3" ? "Saved — personal profiles now sync through your bucket." : "Saved — personal profiles sync through Submarine Cloud.");
      onChanged?.();
    } catch (e) { setErr(clean(e)); }
    finally { setBusy(null); }
  };

  const input =
    "w-full h-9 px-3 bg-zinc-900/60 border border-white/10 rounded-lg text-[12.5px] text-zinc-50 placeholder:text-zinc-600 outline-none focus:border-primary/50 transition-colors font-mono";
  const fieldLabel = "text-[11px] text-zinc-400";
  const tab = (active: boolean) =>
    `h-8 px-3 rounded-md text-[12px] font-semibold flex items-center gap-1.5 transition-colors ${
      active ? "bg-primary text-black" : "bg-zinc-900/60 text-zinc-400 hover:text-zinc-200 border border-white/5"
    }`;

  return (
    <div className="bg-zinc-900/40 border border-white/5 rounded-lg p-3 space-y-3">
      <div className="flex items-center justify-between gap-2 flex-wrap">
        <span className="text-[11px] font-bold uppercase tracking-wider text-zinc-500">Profile sync storage</span>
        <div className="flex items-center gap-1">
          <button onClick={() => { setBackend("http"); setOk(null); }} className={tab(backend === "http")}>
            <Cloud size={12} /> Submarine Cloud
          </button>
          <button onClick={() => { setBackend("s3"); setOk(null); }} className={tab(backend === "s3")}>
            <Database size={12} /> S3 bucket
          </button>
        </div>
      </div>

      {backend === "http" ? (
        <p className="text-[11.5px] text-zinc-500 leading-relaxed">
          Personal profiles sync through your Submarine Cloud account.
        </p>
      ) : (
        <>
          <p className="text-[11.5px] text-zinc-500 leading-relaxed">
            Personal profiles sync item by item through your own S3-compatible bucket (Linode, AWS, MinIO, R2, B2),
            still end-to-end encrypted. The access key stays in an s3cmd config file that Submarine reads when it
            syncs and never copies. Sharing a profile still needs a Submarine Cloud account.
          </p>
          <label className="block space-y-1">
            <span className={fieldLabel}>Credentials file (s3cmd format)</span>
            <input value={s3.credentials_file} onChange={(e) => set({ credentials_file: e.target.value })} className={input} spellCheck={false} />
          </label>
          <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
            <label className="block space-y-1">
              <span className={fieldLabel}>Bucket</span>
              <input value={s3.bucket} onChange={(e) => set({ bucket: e.target.value })} placeholder="my-bucket" className={input} spellCheck={false} />
            </label>
            <label className="block space-y-1">
              <span className={fieldLabel}>Folder in the bucket</span>
              <input value={s3.prefix} onChange={(e) => set({ prefix: e.target.value })} placeholder="submarine/sync" className={input} spellCheck={false} />
            </label>
          </div>

          <button onClick={() => setAdvanced(!advanced)} className="text-[11px] text-zinc-500 hover:text-zinc-300 flex items-center gap-1">
            <ChevronRight size={11} className={`transition-transform ${advanced ? "rotate-90" : ""}`} /> Endpoint and region
          </button>
          {advanced && (
            <div className="space-y-2 pl-3 border-l border-white/5">
              <div className="grid grid-cols-1 sm:grid-cols-2 gap-2">
                <label className="block space-y-1">
                  <span className={fieldLabel}>Endpoint (default: host_base from the file)</span>
                  <input
                    value={s3.endpoint ?? ""}
                    onChange={(e) => set({ endpoint: e.target.value || null })}
                    placeholder={saved?.endpoint ?? "https://…"}
                    className={input}
                    spellCheck={false}
                  />
                </label>
                <label className="block space-y-1">
                  <span className={fieldLabel}>Region (default: detected)</span>
                  <input
                    value={s3.region ?? ""}
                    onChange={(e) => set({ region: e.target.value || null })}
                    placeholder={saved?.region ?? "us-east-1"}
                    className={input}
                    spellCheck={false}
                  />
                </label>
              </div>
              <label className="flex items-center gap-2 text-[11.5px] text-zinc-400">
                <input type="checkbox" checked={s3.path_style} onChange={(e) => set({ path_style: e.target.checked })} />
                Path-style URLs (host/bucket/key) — works with every provider
              </label>
            </div>
          )}

          {saved && saved.s3.credentials_file === s3.credentials_file && (
            saved.credentials_ok ? (
              <div className="flex items-center gap-1.5 text-[11px] text-emerald-300/90">
                <CheckCircle2 size={11} /> Keys found
                {saved.endpoint && <span className="text-zinc-500 font-mono truncate">· {saved.endpoint}</span>}
              </div>
            ) : (
              <div className="flex items-start gap-1.5 text-[11px] text-amber-300/90">
                <AlertTriangle size={11} className="mt-0.5 shrink-0" /> <span className="break-words">{clean(saved.credentials_error)}</span>
              </div>
            )
          )}
        </>
      )}

      {err && <Message kind="err" text={err} />}
      {ok && !err && <Message kind="ok" text={ok} />}

      <div className="flex items-center gap-2 justify-end">
        {backend === "s3" && (
          <button
            onClick={test}
            disabled={busy !== null}
            className="h-8 px-3 rounded-lg bg-white/5 border border-white/10 text-zinc-200 hover:bg-white/10 text-[12px] font-semibold flex items-center gap-1.5 disabled:opacity-40"
          >
            {busy === "test" ? <Loader2 size={12} className="animate-spin" /> : <PlugZap size={12} />} Test connection
          </button>
        )}
        <button
          onClick={save}
          disabled={busy !== null || !dirty}
          className="h-8 px-3 rounded-lg bg-primary text-black text-[12px] font-bold flex items-center gap-1.5 disabled:opacity-40"
        >
          {busy === "save" && <Loader2 size={12} className="animate-spin" />} Save
        </button>
      </div>
    </div>
  );
}

function Message({ kind, text }: { kind: "ok" | "err"; text: string }) {
  return kind === "err" ? (
    <div className="px-3 py-2 rounded-lg bg-rose-500/10 border border-rose-500/30 text-rose-200 text-[12px] flex items-start gap-2">
      <AlertTriangle size={13} className="mt-0.5 shrink-0" /> <span className="flex-1 break-words">{text}</span>
    </div>
  ) : (
    <div className="px-3 py-2 rounded-lg bg-emerald-500/10 border border-emerald-500/30 text-emerald-200 text-[12px] flex items-start gap-2">
      <CheckCircle2 size={13} className="mt-0.5 shrink-0" /> <span className="flex-1 break-words">{text}</span>
    </div>
  );
}
