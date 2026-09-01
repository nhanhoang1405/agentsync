export type FontFamily = "system" | "sans" | "serif" | "mono";

export interface AppearancePreferences {
  fontFamily: FontFamily;
  fontSize: number;
  uiZoom: number;
}

const STORAGE_KEY = "agentsync.appearance.v1";

export const defaultPreferences: AppearancePreferences = {
  fontFamily: "system",
  fontSize: 14,
  uiZoom: 100,
};

export function loadPreferences(): AppearancePreferences {
  try {
    const saved = JSON.parse(localStorage.getItem(STORAGE_KEY) ?? "{}");
    return normalizePreferences(saved);
  } catch {
    return defaultPreferences;
  }
}

export function savePreferences(preferences: AppearancePreferences) {
  const normalized = normalizePreferences(preferences);
  localStorage.setItem(STORAGE_KEY, JSON.stringify(normalized));
  return normalized;
}

export function fontFamilyValue(fontFamily: FontFamily): string {
  switch (fontFamily) {
    case "sans":
      return 'Inter, "Segoe UI", Arial, sans-serif';
    case "serif":
      return 'Georgia, "Times New Roman", serif';
    case "mono":
      return '"JetBrains Mono", "Cascadia Code", Consolas, monospace';
    default:
      return 'ui-sans-serif, system-ui, -apple-system, "Segoe UI", sans-serif';
  }
}

function normalizePreferences(value: Partial<AppearancePreferences>) {
  const allowedFonts: FontFamily[] = ["system", "sans", "serif", "mono"];
  return {
    fontFamily: allowedFonts.includes(value.fontFamily as FontFamily)
      ? value.fontFamily as FontFamily
      : defaultPreferences.fontFamily,
    fontSize: clamp(value.fontSize, 12, 22, defaultPreferences.fontSize),
    uiZoom: clamp(value.uiZoom, 80, 140, defaultPreferences.uiZoom),
  };
}

function clamp(value: unknown, minimum: number, maximum: number, fallback: number) {
  if (typeof value !== "number" || !Number.isFinite(value)) return fallback;
  return Math.min(maximum, Math.max(minimum, Math.round(value)));
}
