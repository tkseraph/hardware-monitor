export type ThemePreference = "system" | "light" | "dark";
export const THEME_KEY = "monitor-theme";
export function parseTheme(value: string | null): ThemePreference {
  return value === "light" || value === "dark" ? value : "system";
}
export function resolveTheme(preference: ThemePreference, systemDark: boolean): "light" | "dark" {
  return preference === "system" ? (systemDark ? "dark" : "light") : preference;
}
export function readTheme(): ThemePreference {
  try { return parseTheme(localStorage.getItem(THEME_KEY)); }
  catch { return "system"; }
}
export function applyTheme(preference: ThemePreference): void {
  document.documentElement.dataset.theme = resolveTheme(preference, window.matchMedia("(prefers-color-scheme: dark)").matches);
}
