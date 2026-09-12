import { useEffect, useState } from "react";
import { applyTheme, readTheme, THEME_KEY } from "./theme";
import type { ThemePreference } from "./theme";

export function useTheme() {
  const [preference, setPreference] = useState<ThemePreference>(readTheme);
  useEffect(() => {
    applyTheme(preference);
    try { localStorage.setItem(THEME_KEY, preference); } catch { /* Session-only when storage is unavailable. */ }
    const media = window.matchMedia("(prefers-color-scheme: dark)");
    const sync = () => applyTheme(preference);
    media.addEventListener("change", sync);
    return () => media.removeEventListener("change", sync);
  }, [preference]);
  return [preference, setPreference] as const;
}
