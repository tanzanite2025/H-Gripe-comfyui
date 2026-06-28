import { useEffect, useState } from "react";
import { listProfiles, type ProviderProfile } from "../bridge/tauri";

interface ProfilePickerProps {
  /** Applies the chosen profile's provider / model / credentials to the node. */
  onApply: (profile: ProviderProfile) => void;
}

// Lets a generate node adopt an H-Gripe provider profile (the same ones the
// PSD Studio tab uses), filling provider / model / credentials_ref in one step.
export function ProfilePicker({ onApply }: ProfilePickerProps) {
  const [profiles, setProfiles] = useState<ProviderProfile[]>([]);
  const [ref, setRef] = useState("");
  const [error, setError] = useState<string | null>(null);

  useEffect(() => {
    let cancelled = false;
    listProfiles()
      .then((p) => !cancelled && setProfiles(p))
      .catch((e) => !cancelled && setError(String(e)));
    return () => {
      cancelled = true;
    };
  }, []);

  return (
    <label className="field">
      <span>Profile</span>
      <select
        value={ref}
        onChange={(e) => {
          setRef(e.target.value);
          const p = profiles.find((x) => x.profile_ref === e.target.value);
          if (p) onApply(p);
        }}
      >
        <option value="">— pick a profile —</option>
        {profiles.map((p) => (
          <option key={p.profile_ref} value={p.profile_ref}>
            {p.profile_ref}
            {p.provider ? ` (${p.provider})` : ""}
          </option>
        ))}
      </select>
      {error ? (
        <small className="hint">profiles unavailable: {error}</small>
      ) : (
        <small className="hint">fills provider / model / credentials from H-Gripe profiles</small>
      )}
    </label>
  );
}
