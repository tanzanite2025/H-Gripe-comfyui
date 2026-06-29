// History tab: list / view / rerun / clean up recorded tasks.

import { useCallback, useState } from "react";

import {
  commands,
  pretty,
  type CleanupOptions,
  type HistoryQuery,
  type HistoryRecord,
} from "../bridge/desktop";
import { useT } from "../i18n";
import { useToast } from "./common";

export function HistoryPanel() {
  const t = useT();
  const toast = useToast();
  const [provider, setProvider] = useState("");
  const [statusFilter, setStatusFilter] = useState("");
  const [limit, setLimit] = useState("50");
  const [records, setRecords] = useState<HistoryRecord[] | null>(null);
  const [listErr, setListErr] = useState<string | null>(null);
  const [detail, setDetail] = useState("");
  const [keepLatest, setKeepLatest] = useState("");
  const [deleteFiles, setDeleteFiles] = useState(false);
  const [cleanupOut, setCleanupOut] = useState("");

  const load = useCallback(async () => {
    const query: HistoryQuery = {
      limit: parseInt(limit || "50", 10),
      provider: provider.trim() || null,
      operation: null,
      status: statusFilter || null,
      has_output_files: null,
    };
    try {
      setListErr(null);
      setRecords(await commands.listHistory(query));
    } catch (err) {
      setRecords(null);
      setListErr(String(err));
    }
  }, [limit, provider, statusFilter]);

  const showDetail = useCallback(async (taskId: string) => {
    try {
      setDetail(pretty(await commands.historyDetail(taskId)));
    } catch (err) {
      setDetail(String(err));
    }
  }, []);

  const rerun = useCallback(
    async (taskId: string) => {
      toast(t("history.rerunning", { id: taskId }));
      try {
        const result = await commands.rerunTask(taskId, true);
        setDetail(pretty(result));
        toast(
          t("history.rerunDone", { status: result.status }),
          result.status === "failed" ? "err" : "ok",
        );
        void load();
      } catch (err) {
        toast(String(err), "err");
      }
    },
    [load, t, toast],
  );

  const cleanupOptions = useCallback((): CleanupOptions => {
    const keep = keepLatest.trim();
    return {
      keep_latest: keep === "" ? null : parseInt(keep, 10),
      older_than_timestamp_ms: null,
      provider: provider.trim() || null,
      operation: null,
      status: statusFilter || null,
      has_output_files: null,
      delete_all_matched: false,
      delete_output_files: deleteFiles,
    };
  }, [keepLatest, provider, statusFilter, deleteFiles]);

  const preview = useCallback(async () => {
    try {
      setCleanupOut(pretty(await commands.historyCleanupPreview(cleanupOptions())));
    } catch (err) {
      setCleanupOut(String(err));
    }
  }, [cleanupOptions]);

  const apply = useCallback(async () => {
    try {
      setCleanupOut(pretty(await commands.historyCleanupApply(cleanupOptions())));
      toast(t("cleanup.applied"), "ok");
      void load();
    } catch (err) {
      setCleanupOut(String(err));
      toast(String(err), "err");
    }
  }, [cleanupOptions, load, t, toast]);

  return (
    <>
      <div className="row wrap">
        <h2>{t("history.heading")}</h2>
        <input
          placeholder={t("history.providerFilter")}
          value={provider}
          onChange={(e) => setProvider(e.target.value)}
        />
        <select value={statusFilter} onChange={(e) => setStatusFilter(e.target.value)}>
          <option value="">{t("history.anyStatus")}</option>
          <option value="succeeded">{t("history.statusSucceeded")}</option>
          <option value="failed">{t("history.statusFailed")}</option>
          <option value="cached">{t("history.statusCached")}</option>
          <option value="cancelled">{t("history.statusCancelled")}</option>
        </select>
        <input
          type="number"
          min={1}
          style={{ width: 80 }}
          value={limit}
          onChange={(e) => setLimit(e.target.value)}
        />
        <button className="primary" onClick={() => void load()}>
          {t("btn.load")}
        </button>
      </div>
      <table>
        <thead>
          <tr>
            <th>{t("history.colTime")}</th>
            <th>{t("history.colProvider")}</th>
            <th>{t("history.colOperation")}</th>
            <th>{t("history.colStatus")}</th>
            <th>{t("history.colFiles")}</th>
            <th></th>
          </tr>
        </thead>
        <tbody>
          {listErr ? (
            <tr>
              <td colSpan={6} className="status err">
                {listErr}
              </td>
            </tr>
          ) : records && records.length === 0 ? (
            <tr>
              <td colSpan={6}>{t("history.noRecords")}</td>
            </tr>
          ) : (
            (records ?? []).map((r) => (
              <tr key={r.task_id}>
                <td>{new Date(Number(r.timestamp_ms)).toLocaleString()}</td>
                <td>{r.provider}</td>
                <td>{r.operation}</td>
                <td>{r.status}</td>
                <td>{r.output_file_count}</td>
                <td>
                  <button onClick={() => void showDetail(r.task_id)}>{t("history.view")}</button>{" "}
                  <button onClick={() => void rerun(r.task_id)}>{t("history.rerun")}</button>
                </td>
              </tr>
            ))
          )}
        </tbody>
      </table>
      <h3>{t("history.detail")}</h3>
      <pre className="json">{detail}</pre>
      <h3>{t("history.cleanup")}</h3>
      <div className="row wrap">
        <label>
          <span>{t("cleanup.keepLatest")}</span>{" "}
          <input
            type="number"
            min={0}
            style={{ width: 80 }}
            value={keepLatest}
            onChange={(e) => setKeepLatest(e.target.value)}
          />
        </label>
        <label>
          <input
            type="checkbox"
            checked={deleteFiles}
            onChange={(e) => setDeleteFiles(e.target.checked)}
          />{" "}
          <span>{t("cleanup.deleteFiles")}</span>
        </label>
        <button onClick={() => void preview()}>{t("cleanup.preview")}</button>
        <button className="danger" onClick={() => void apply()}>
          {t("cleanup.apply")}
        </button>
      </div>
      <pre className="json">{cleanupOut}</pre>
    </>
  );
}
