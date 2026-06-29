// Credentials / Profiles tabs: summary cards, validation, and in-place editing
// of credentials.json / provider_profiles.json.

import { useCallback, useEffect, useState } from "react";

import {
  commands,
  pretty,
  type ConfigKind,
  type CredentialSummary,
  type ProfileSummary,
  type ValidationResult,
} from "../bridge/desktop";
import { useT } from "../i18n";
import { emptyStatus, Status, useToast, type StatusState } from "./common";

function CredentialCards({ items }: { items: CredentialSummary[] }) {
  const t = useT();
  if (!items.length) {
    return (
      <div className="card">
        <div className="value">{t("common.noEntries")}</div>
      </div>
    );
  }
  return (
    <>
      {items.map((c) => (
        <div className="card" key={c.credential_ref}>
          <div className="label">{c.credential_ref}</div>
          <div className="value">
            {t("field.provider")}: {c.provider ?? "-"}
            <br />
            {t("field.key")}:{" "}
            {c.api_key_configured
              ? t("creds.keySet")
              : c.api_key_env
                ? t("creds.keyEnv") + c.api_key_env
                : t("creds.keyNone")}
            <br />
            {t("field.headers")}: {c.headers_count}
          </div>
        </div>
      ))}
    </>
  );
}

function ProfileCards({ items }: { items: ProfileSummary[] }) {
  const t = useT();
  if (!items.length) {
    return (
      <div className="card">
        <div className="value">{t("common.noEntries")}</div>
      </div>
    );
  }
  return (
    <>
      {items.map((p) => (
        <div className="card" key={p.profile_ref}>
          <div className="label">{p.profile_ref}</div>
          <div className="value">
            {t("field.provider")}: {p.provider ?? "-"}
            <br />
            {t("field.model")}: {p.model ?? "-"}
            <br />
            {t("field.creds")}: {p.credentials_ref ?? "-"}
            <br />
            {t("field.params")}: {p.params_count}
          </div>
        </div>
      ))}
    </>
  );
}

export function ConfigPanel({ kind }: { kind: ConfigKind }) {
  const t = useT();
  const toast = useToast();
  const [creds, setCreds] = useState<CredentialSummary[]>([]);
  const [profiles, setProfiles] = useState<ProfileSummary[]>([]);
  const [summaryErr, setSummaryErr] = useState<string | null>(null);
  const [editor, setEditor] = useState("");
  const [status, setStatus] = useState<StatusState>(emptyStatus);
  const [validation, setValidation] = useState<ValidationResult | null>(null);
  const [validationErr, setValidationErr] = useState<string | null>(null);

  const load = useCallback(async () => {
    try {
      setSummaryErr(null);
      if (kind === "credentials") setCreds(await commands.getCredentials());
      else setProfiles(await commands.getProfiles());
    } catch (err) {
      setSummaryErr(String(err));
    }
    try {
      setEditor(await commands.readConfigFile(kind));
    } catch (err) {
      toast(String(err), "err");
    }
  }, [kind, toast]);

  useEffect(() => {
    void load();
  }, [load]);

  const save = useCallback(async () => {
    setStatus({ text: t("status.saving"), kind: "" });
    try {
      await commands.writeConfigFile(kind, editor);
      setStatus({ text: t("status.saved"), kind: "ok" });
      toast(t("toast.savedKind", { kind }), "ok");
      void load();
    } catch (err) {
      setStatus({ text: String(err), kind: "err" });
      toast(String(err), "err");
    }
  }, [editor, kind, load, t, toast]);

  const validate = useCallback(async () => {
    try {
      setValidationErr(null);
      setValidation(
        kind === "credentials"
          ? await commands.checkCredentials()
          : await commands.checkProfiles(),
      );
    } catch (err) {
      setValidation(null);
      setValidationErr(String(err));
    }
  }, [kind]);

  const issues = validation?.issues ?? [];
  const ok = validation ? (validation.ok ?? issues.length === 0) : false;

  return (
    <>
      <div className="row">
        <h2>{kind === "credentials" ? t("creds.heading") : t("profiles.heading")}</h2>
        <button onClick={() => void validate()}>{t("btn.validate")}</button>
      </div>
      <div className="cards">
        {summaryErr ? (
          <div className="card">
            <div className="value missing">{summaryErr}</div>
          </div>
        ) : kind === "credentials" ? (
          <CredentialCards items={creds} />
        ) : (
          <ProfileCards items={profiles} />
        )}
      </div>
      <div className="validation">
        {validationErr && <span className="badge err">{validationErr}</span>}
        {validation && (
          <>
            <span className={"badge " + (ok ? "ok" : "err")}>
              {ok ? t("validation.valid") : t("validation.issues", { count: issues.length })}
            </span>
            {issues.length > 0 && <pre className="json">{pretty(issues)}</pre>}
          </>
        )}
      </div>
      <h3>{kind === "credentials" ? t("creds.file") : t("profiles.file")}</h3>
      <textarea
        spellCheck={false}
        value={editor}
        onChange={(e) => setEditor(e.target.value)}
      />
      <div className="row">
        <button onClick={() => void load()}>{t("btn.reload")}</button>
        <button className="primary" onClick={() => void save()}>
          {t("btn.save")}
        </button>
        <Status status={status} />
      </div>
    </>
  );
}
