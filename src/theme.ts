// 外观设置：主题（明亮 / 暗色 / 跟随系统）、字号、界面语言。
// 这些设置都落在 config.ui 上，由 App.vue 在启动时和保存后调用。

import { setLang } from "./i18n";

export type ThemeMode = "light" | "dark" | "system";
export type Lang = "zh" | "en";

let mediaQuery: MediaQueryList | null = null;
let mediaListener: ((event: MediaQueryListEvent) => void) | null = null;
let currentMode: ThemeMode = "system";

function effectiveTheme(mode: ThemeMode): "light" | "dark" {
  if (mode === "dark") return "dark";
  if (mode === "light") return "light";
  if (typeof window !== "undefined" && window.matchMedia) {
    return window.matchMedia("(prefers-color-scheme: dark)").matches ? "dark" : "light";
  }
  return "light";
}

/** 写入 <html data-theme>，并（在跟随系统时）订阅系统主题变化。 */
export function applyTheme(mode: string) {
  const next: ThemeMode = mode === "dark" || mode === "light" ? mode : "system";
  currentMode = next;
  const root = document.documentElement;
  root.dataset.theme = effectiveTheme(next);
  root.dataset.themeMode = next;

  if (mediaListener && mediaQuery) {
    mediaQuery.removeEventListener("change", mediaListener);
    mediaListener = null;
  }
  if (next === "system" && window.matchMedia) {
    mediaQuery = window.matchMedia("(prefers-color-scheme: dark)");
    mediaListener = () => {
      if (currentMode === "system") root.dataset.theme = effectiveTheme("system");
    };
    mediaQuery.addEventListener("change", mediaListener);
  }
}

/** 字号：作用在 :root 上，各组件用 rem/继承的都能跟着变。 */
export function applyFontSize(size: number | undefined) {
  const px = Math.min(20, Math.max(12, Math.round(Number(size) || 14)));
  document.documentElement.style.fontSize = `${px}px`;
}

/** 界面语言：主要影响 <html lang> 与少数关键文案（i18n.ts 里的词条）。 */
export function applyLanguage(lang: string) {
  const next: Lang = lang === "en" ? "en" : "zh";
  document.documentElement.lang = next === "en" ? "en" : "zh-CN";
  document.documentElement.dataset.lang = next;
  // 同步给 i18n：组件里的 t() 依赖这个响应式状态，切换后立刻重新渲染。
  setLang(next);
  try {
    localStorage.setItem("deepaisee.lang", next);
  } catch {
    /* 隐私模式等情况下忽略 */
  }
}

/** 一次性应用整套外观设置。 */
export function applyAppearance(ui: { theme?: string; font_size?: number; language?: string } | null | undefined) {
  applyTheme(ui?.theme ?? "system");
  applyFontSize(ui?.font_size);
  applyLanguage(ui?.language ?? "zh");
}

/** 启动时先按本地缓存渲染，避免首帧闪白/闪黑，等配置读回来再校正。 */
export function primeAppearance() {
  let theme = "system";
  let lang = "zh";
  try {
    theme = localStorage.getItem("deepaisee.theme") || "system";
    lang = localStorage.getItem("deepaisee.lang") || "zh";
  } catch {}
  applyTheme(theme);
  applyLanguage(lang);
}

export function rememberTheme(mode: string) {
  try {
    localStorage.setItem("deepaisee.theme", mode);
  } catch {}
}
